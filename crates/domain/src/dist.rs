//! Distribution rules: release versions, platform targets, release asset
//! names and the pure edits of a `PATH` list. No I/O.

use std::cmp::Ordering;
use std::fmt;
use std::path::PathBuf;

/// A release version such as `0.5.0` (a leading `v` is accepted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    /// Parses `0.5.0` or `v0.5.0`. Pre-release and build parts are refused:
    /// self-update only follows full releases.
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let mut parts = text.split('.');
        let major = number(parts.next()?)?;
        let minor = number(parts.next()?)?;
        let patch = number(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }
        Some(Version {
            major,
            minor,
            patch,
        })
    }

    /// The git tag of this release.
    pub fn tag(&self) -> String {
        format!("v{self}")
    }
}

fn number(part: &str) -> Option<u64> {
    if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch))
    }
}

/// A platform swarmux is released for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    WindowsX64,
    MacArm64,
    MacX64,
    LinuxX64,
}

impl Target {
    pub const ALL: [Target; 4] = [
        Target::WindowsX64,
        Target::MacArm64,
        Target::MacX64,
        Target::LinuxX64,
    ];

    /// The target for an operating system and architecture as
    /// `std::env::consts` names them.
    pub fn from_parts(os: &str, arch: &str) -> Option<Target> {
        match (os, arch) {
            ("windows", "x86_64") => Some(Target::WindowsX64),
            ("macos", "aarch64") => Some(Target::MacArm64),
            ("macos", "x86_64") => Some(Target::MacX64),
            ("linux", "x86_64") => Some(Target::LinuxX64),
            _ => None,
        }
    }

    /// The Rust target triple.
    pub fn triple(self) -> &'static str {
        match self {
            Target::WindowsX64 => "x86_64-pc-windows-msvc",
            Target::MacArm64 => "aarch64-apple-darwin",
            Target::MacX64 => "x86_64-apple-darwin",
            Target::LinuxX64 => "x86_64-unknown-linux-gnu",
        }
    }

    /// `zip` on Windows, `tar.gz` elsewhere.
    pub fn archive_ext(self) -> &'static str {
        match self {
            Target::WindowsX64 => "zip",
            _ => "tar.gz",
        }
    }

    /// The file name of the executable.
    pub fn exe_name(self) -> &'static str {
        match self {
            Target::WindowsX64 => "swarmux.exe",
            _ => "swarmux",
        }
    }

    /// The release archive of this target. The name carries no version, so
    /// `releases/latest/download/<name>` always points at the newest one.
    pub fn asset_name(self) -> String {
        format!("swarmux-{}.{}", self.triple(), self.archive_ext())
    }

    /// The detached minisign signature that goes with [`Self::asset_name`].
    pub fn signature_name(self) -> String {
        format!("{}.minisig", self.asset_name())
    }
}

/// What `swarmux install` set up, written beside the data so that
/// `swarmux uninstall` removes exactly that and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Manifest {
    /// The installed executable.
    pub binary: PathBuf,
    /// The outermost folder that did not exist before the install made it.
    pub created_root: Option<PathBuf>,
    /// The `PATH` entry the install added, when it did.
    pub path_entry: Option<String>,
    /// Whether the install created the data folder (`~/.swarmux`).
    pub data_dir_created: bool,
}

/// `list` with `entry` appended, or `None` when it is already there.
/// `sep` is the list separator; folders compare without regard to a
/// trailing slash, and without regard to case when `fold_case` is set.
pub fn path_list_with(list: &str, entry: &str, sep: char, fold_case: bool) -> Option<String> {
    if path_list_has(list, entry, sep, fold_case) {
        return None;
    }
    if list.trim().is_empty() {
        return Some(entry.to_string());
    }
    let list = list.trim_end_matches(sep);
    Some(format!("{list}{sep}{entry}"))
}

/// `list` without `entry`, or `None` when it was not there.
pub fn path_list_without(list: &str, entry: &str, sep: char, fold_case: bool) -> Option<String> {
    if !path_list_has(list, entry, sep, fold_case) {
        return None;
    }
    let kept: Vec<&str> = list
        .split(sep)
        .filter(|p| !same_folder(p, entry, fold_case))
        .collect();
    Some(kept.join(&sep.to_string()))
}

/// Whether `list` holds `entry`.
pub fn path_list_has(list: &str, entry: &str, sep: char, fold_case: bool) -> bool {
    list.split(sep).any(|p| same_folder(p, entry, fold_case))
}

fn same_folder(a: &str, b: &str, fold_case: bool) -> bool {
    let a = a.trim().trim_end_matches(['\\', '/']);
    let b = b.trim().trim_end_matches(['\\', '/']);
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if fold_case {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_order() {
        let v = Version::parse("v0.10.2").unwrap();
        assert_eq!(v.to_string(), "0.10.2");
        assert_eq!(v.tag(), "v0.10.2");
        assert!(v > Version::parse("0.9.9").unwrap());
        assert!(Version::parse("1.0.0-rc1").is_none());
        assert!(Version::parse("1.0").is_none());
        assert!(Version::parse("1.0.0.0").is_none());
        assert!(Version::parse("a.b.c").is_none());
    }

    #[test]
    fn targets_name_their_assets() {
        let t = Target::from_parts("windows", "x86_64").unwrap();
        assert_eq!(t.asset_name(), "swarmux-x86_64-pc-windows-msvc.zip");
        assert_eq!(t.exe_name(), "swarmux.exe");
        let t = Target::from_parts("macos", "aarch64").unwrap();
        assert_eq!(
            t.signature_name(),
            "swarmux-aarch64-apple-darwin.tar.gz.minisig"
        );
        assert!(Target::from_parts("linux", "riscv64").is_none());
    }

    #[test]
    fn path_lists_gain_and_lose_an_entry() {
        let with = path_list_with(r"C:\a;C:\b", r"C:\swarmux\bin", ';', true).unwrap();
        assert_eq!(with, r"C:\a;C:\b;C:\swarmux\bin");
        assert!(path_list_with(&with, r"c:\SWARMUX\bin\", ';', true).is_none());
        assert_eq!(
            path_list_without(&with, r"C:\swarmux\bin", ';', true).unwrap(),
            r"C:\a;C:\b"
        );
        assert!(path_list_without(r"C:\a", r"C:\swarmux\bin", ';', true).is_none());
        assert_eq!(path_list_with("", "/x", ':', false).unwrap(), "/x");
        assert!(!path_list_has("/a:/X", "/x", ':', false));
    }
}
