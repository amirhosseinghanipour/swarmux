use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

/// The platform's conventions for where configuration and state live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// Linux and the other Unixes: the XDG base directories.
    Unix,
    Mac,
    Windows,
}

impl Platform {
    pub fn current() -> Platform {
        if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::Mac
        } else {
            Platform::Unix
        }
    }
}

/// Why a folder was chosen, in words `swarmux config paths` prints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// An environment variable named it.
    Explicit(&'static str),
    /// `~/.swarmux` exists, so an install that has it keeps using it.
    Existing,
    /// The platform's convention; the text names the rule.
    Platform(String),
    /// Fixed by the caller (tests, the demo).
    Fixed,
}

impl Why {
    pub fn text(&self) -> String {
        match self {
            Why::Explicit(var) => format!("${var} is set"),
            Why::Existing => "~/.swarmux exists, so it keeps being used (nothing moves)".into(),
            Why::Platform(rule) => rule.clone(),
            Why::Fixed => "fixed by the caller".into(),
        }
    }
}

/// A folder (or file) and why it was chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Located {
    pub dir: PathBuf,
    pub why: Why,
}

/// Where swarmux keeps its data. Injectable so tests never touch the real home.
///
/// Configuration (what a person edits) and state (what swarmux writes: the
/// database, the ledger, the key, hidden checks, the verifier's copies, the
/// spools) are two folders. An install that already has `~/.swarmux` keeps
/// both there: nothing moves.
#[derive(Clone, Debug)]
pub struct Paths {
    /// The user's home folder.
    pub home: PathBuf,
    /// Where `config.toml` and the other files a person edits live.
    pub config_dir: PathBuf,
    /// Where swarmux's own data lives (the state folder).
    pub data_dir: PathBuf,
    pub db: PathBuf,
    pub config: PathBuf,
    /// Hidden checks, per repository: never inside a repository.
    pub holdout: PathBuf,
    /// Clean copies the verifier makes and their shared build folders.
    pub verify: PathBuf,
    /// The organisation's file, which can lock keys.
    pub managed: PathBuf,
    /// How the folders above were chosen.
    pub config_why: Why,
    pub state_why: Why,
    pub managed_why: Why,
}

/// What the environment says: `SWARMUX_CONFIG_DIR`, `XDG_CONFIG_HOME` and
/// the like, read through one function so tests can supply their own.
pub type Env<'a> = &'a dyn Fn(&str) -> Option<OsString>;

fn non_empty(env: Env<'_>, name: &str) -> Option<PathBuf> {
    env(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// An absolute path from the environment (XDG says relative values are
/// invalid and must be ignored).
fn absolute(env: Env<'_>, name: &str) -> Option<PathBuf> {
    non_empty(env, name).filter(|p| p.is_absolute())
}

fn located(dir: PathBuf, why: &str) -> Located {
    Located {
        dir,
        why: Why::Platform(why.into()),
    }
}

fn config_dir_for(
    env: Env<'_>,
    home: &Path,
    platform: Platform,
    exists: &dyn Fn(&Path) -> bool,
) -> Located {
    // COMPAT(legacy-home, 0.1.0, 2026-11-14): an install that has ~/.swarmux keeps
    // using it for config and state, for good (not a deprecation).
    let legacy = home.join(".swarmux");
    if let Some(dir) = non_empty(env, "SWARMUX_CONFIG_DIR") {
        return Located {
            dir,
            why: Why::Explicit("SWARMUX_CONFIG_DIR"),
        };
    }
    if exists(&legacy) {
        return Located {
            dir: legacy,
            why: Why::Existing,
        };
    }
    match platform {
        Platform::Unix => match absolute(env, "XDG_CONFIG_HOME") {
            Some(x) => located(x.join("swarmux"), "$XDG_CONFIG_HOME/swarmux"),
            None => located(
                home.join(".config").join("swarmux"),
                "~/.config/swarmux (the XDG default)",
            ),
        },
        Platform::Mac => {
            let native = home
                .join("Library")
                .join("Application Support")
                .join("swarmux");
            let xdg = absolute(env, "XDG_CONFIG_HOME")
                .unwrap_or_else(|| home.join(".config"))
                .join("swarmux");
            if !exists(&native) && exists(&xdg) {
                located(xdg, "~/.config/swarmux exists (accepted on macOS)")
            } else {
                located(native, "~/Library/Application Support/swarmux")
            }
        }
        Platform::Windows => match non_empty(env, "APPDATA") {
            Some(a) => located(a.join("swarmux"), "%APPDATA%\\swarmux"),
            None => located(
                home.join("AppData").join("Roaming").join("swarmux"),
                "%APPDATA% is not set: ~\\AppData\\Roaming\\swarmux",
            ),
        },
    }
}

fn state_dir_for(
    env: Env<'_>,
    home: &Path,
    platform: Platform,
    exists: &dyn Fn(&Path) -> bool,
) -> Located {
    let legacy = home.join(".swarmux");
    if let Some(dir) = non_empty(env, "SWARMUX_STATE_DIR") {
        return Located {
            dir,
            why: Why::Explicit("SWARMUX_STATE_DIR"),
        };
    }
    if exists(&legacy) {
        return Located {
            dir: legacy,
            why: Why::Existing,
        };
    }
    match platform {
        Platform::Unix => match absolute(env, "XDG_STATE_HOME") {
            Some(x) => located(x.join("swarmux"), "$XDG_STATE_HOME/swarmux"),
            None => located(
                home.join(".local").join("state").join("swarmux"),
                "~/.local/state/swarmux (the XDG default)",
            ),
        },
        Platform::Mac => located(
            home.join("Library")
                .join("Application Support")
                .join("swarmux")
                .join("state"),
            "~/Library/Application Support/swarmux/state",
        ),
        Platform::Windows => match non_empty(env, "LOCALAPPDATA") {
            Some(a) => located(a.join("swarmux"), "%LOCALAPPDATA%\\swarmux"),
            None => located(
                home.join("AppData").join("Local").join("swarmux"),
                "%LOCALAPPDATA% is not set: ~\\AppData\\Local\\swarmux",
            ),
        },
    }
}

fn managed_file_for(env: Env<'_>, platform: Platform) -> Located {
    if let Some(file) = non_empty(env, "SWARMUX_MANAGED_FILE") {
        return Located {
            dir: file,
            why: Why::Explicit("SWARMUX_MANAGED_FILE"),
        };
    }
    if platform == Platform::Windows {
        let base =
            non_empty(env, "ProgramData").unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        located(
            base.join("swarmux").join("managed.toml"),
            "%ProgramData%\\swarmux\\managed.toml",
        )
    } else {
        located(
            PathBuf::from("/etc/swarmux/managed.toml"),
            "/etc/swarmux/managed.toml",
        )
    }
}

/// Chooses the configuration folder, the state folder and the managed file
/// for `home` on `platform`. `exists` says whether a folder is there.
///
/// Configuration: `SWARMUX_CONFIG_DIR`; else `~/.swarmux` if it exists; else
/// the platform's convention (`$XDG_CONFIG_HOME/swarmux` or
/// `~/.config/swarmux`; on macOS `~/Library/Application Support/swarmux`,
/// with `~/.config/swarmux` accepted when it exists; `%APPDATA%\swarmux`).
/// State: `SWARMUX_STATE_DIR`; else `~/.swarmux` if it exists; else
/// `$XDG_STATE_HOME/swarmux` (`~/.local/state/swarmux`), on macOS
/// `~/Library/Application Support/swarmux/state`, `%LOCALAPPDATA%\swarmux`.
pub fn locate(
    env: Env<'_>,
    home: &Path,
    platform: Platform,
    exists: &dyn Fn(&Path) -> bool,
) -> (Located, Located, Located) {
    (
        config_dir_for(env, home, platform, exists),
        state_dir_for(env, home, platform, exists),
        managed_file_for(env, platform),
    )
}

impl Paths {
    /// Everything under one folder, `~/.swarmux`: what an existing install
    /// has, and what tests and the demo use. No managed file is read.
    pub fn from_home(home: &Path) -> Paths {
        let dir = home.join(".swarmux");
        let fixed = |dir: PathBuf| Located {
            dir,
            why: Why::Fixed,
        };
        Paths::from_parts(
            home,
            fixed(dir.clone()),
            fixed(dir),
            fixed(home.join(".swarmux-no-managed-file")),
        )
    }

    fn from_parts(home: &Path, config: Located, state: Located, managed: Located) -> Paths {
        Paths {
            home: home.to_path_buf(),
            db: state.dir.join("swarmux.db"),
            config: config.dir.join("config.toml"),
            holdout: state.dir.join("holdout"),
            verify: state.dir.join("verify"),
            config_dir: config.dir,
            data_dir: state.dir,
            managed: managed.dir,
            config_why: config.why,
            state_why: state.why,
            managed_why: managed.why,
        }
    }

    /// The folders for `home` as the environment and the file system say.
    pub fn resolve(home: &Path, env: Env<'_>, platform: Platform) -> Paths {
        let (config, state, managed) = locate(env, home, platform, &|p| p.is_dir());
        Paths::from_parts(home, config, state, managed)
    }

    pub fn default_home() -> io::Result<Paths> {
        let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        let home = std::env::var_os(var)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{var} is not set")))?;
        Ok(Paths::resolve(
            Path::new(&home),
            &|name| std::env::var_os(name),
            Platform::current(),
        ))
    }
}

/// Paths at or past this many characters need the extended form on Windows,
/// a little under the 260 of `MAX_PATH` so that names added later still fit.
#[cfg(windows)]
const LONG_FROM: usize = 240;

/// `path` in a form file operations accept past 260 characters.
///
/// On Windows a long absolute path gets the `\\?\` prefix (`\\?\UNC\` for
/// network shares). Short paths, relative paths, paths already prefixed and
/// paths with `..` are returned as they are. Use it for file operations
/// only: programs started with such a folder as their working directory
/// often fail, so children get the plain path. Elsewhere it changes nothing.
pub fn long(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        extended(path)
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

#[cfg(windows)]
fn extended(path: &Path) -> PathBuf {
    use std::path::{Component, Prefix};

    if path.to_string_lossy().chars().count() < LONG_FROM || !path.is_absolute() {
        return path.to_path_buf();
    }
    let mut parts = path.components();
    let head = match parts.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(letter) => format!(r"\\?\{}:", char::from(letter)),
            Prefix::UNC(server, share) => format!(
                r"\\?\UNC\{}\{}",
                server.to_string_lossy(),
                share.to_string_lossy()
            ),
            // Already extended, or a device path.
            _ => return path.to_path_buf(),
        },
        _ => return path.to_path_buf(),
    };
    let mut out = PathBuf::from(head);
    for part in parts {
        match part {
            Component::RootDir => out.push(r"\"),
            Component::Normal(name) => out.push(name),
            Component::CurDir => {}
            // `..` is not understood after the prefix.
            _ => return path.to_path_buf(),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_and_relative_paths_are_left_alone() {
        assert_eq!(long(Path::new("a/b")), Path::new("a/b"));
        let short = std::env::temp_dir().join("x");
        assert_eq!(long(&short), short);
    }

    #[cfg(windows)]
    #[test]
    fn long_windows_paths_get_the_extended_prefix() {
        let deep = format!(r"C:\work\{}\file.txt", "d".repeat(250));
        let ext = long(Path::new(&deep));
        assert_eq!(ext.to_string_lossy(), format!(r"\\?\{deep}"));
        assert_eq!(long(&ext), ext, "already extended");
        let unc = format!(r"\\srv\share\{}", "d".repeat(250));
        assert!(
            long(Path::new(&unc))
                .to_string_lossy()
                .starts_with(r"\\?\UNC\srv\share\d")
        );
        let dotdot = format!(r"C:\work\{}\..\x", "d".repeat(250));
        assert_eq!(long(Path::new(&dotdot)), Path::new(&dotdot));
    }

    #[cfg(windows)]
    #[test]
    fn folders_past_max_path_can_be_made_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path().join("a_long_folder_name_to_pass_max_path");
        let mut deep = top.clone();
        while deep.to_string_lossy().len() < 300 {
            deep.push("a_long_folder_name_to_pass_max_path");
        }
        std::fs::create_dir_all(long(&deep)).unwrap();
        std::fs::write(long(&deep.join("f.txt")), "x").unwrap();
        crate::worktree::remove_dir_retrying(&top).unwrap();
        assert!(!long(&top).exists());
    }

    #[test]
    fn data_lives_under_dot_swarmux() {
        let p = Paths::from_home(Path::new("/home/u"));
        assert_eq!(p.data_dir, Path::new("/home/u/.swarmux"));
        assert_eq!(p.db, Path::new("/home/u/.swarmux/swarmux.db"));
        assert_eq!(p.config, Path::new("/home/u/.swarmux/config.toml"));
        assert_eq!(p.holdout, Path::new("/home/u/.swarmux/holdout"));
        assert_eq!(p.verify, Path::new("/home/u/.swarmux/verify"));
    }

    fn no_env(_: &str) -> Option<OsString> {
        None
    }

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn a_fresh_linux_home_splits_config_and_state() {
        let home = Path::new("/home/u");
        let (config, state, _) = locate(&no_env, home, Platform::Unix, &|_| false);
        assert_eq!(config.dir, Path::new("/home/u/.config/swarmux"));
        assert_eq!(state.dir, Path::new("/home/u/.local/state/swarmux"));
        let p = Paths::resolve(home, &no_env, Platform::Unix);
        assert_eq!(p.config, Path::new("/home/u/.config/swarmux/config.toml"));
        assert_eq!(p.db, Path::new("/home/u/.local/state/swarmux/swarmux.db"));
        assert_eq!(p.holdout, Path::new("/home/u/.local/state/swarmux/holdout"));
        assert_eq!(p.managed, Path::new("/etc/swarmux/managed.toml"));
    }

    #[test]
    fn xdg_variables_are_followed_when_absolute() {
        let env = env_of(&[
            ("XDG_CONFIG_HOME", "/x/cfg"),
            ("XDG_STATE_HOME", "/x/state"),
        ]);
        let (config, state, _) = locate(&env, Path::new("/home/u"), Platform::Unix, &|_| false);
        assert_eq!(config.dir, Path::new("/x/cfg/swarmux"));
        assert_eq!(state.dir, Path::new("/x/state/swarmux"));
        // A relative value is invalid by the spec and is ignored.
        let env = env_of(&[("XDG_CONFIG_HOME", "cfg")]);
        let (config, ..) = locate(&env, Path::new("/home/u"), Platform::Unix, &|_| false);
        assert_eq!(config.dir, Path::new("/home/u/.config/swarmux"));
    }

    #[test]
    fn an_existing_dot_swarmux_keeps_everything_where_it_is() {
        let home = Path::new("/home/u");
        let (config, state, _) = locate(&no_env, home, Platform::Unix, &|p| {
            p == home.join(".swarmux")
        });
        assert_eq!(config.dir, Path::new("/home/u/.swarmux"));
        assert_eq!(config.why, Why::Existing);
        assert_eq!(state.dir, Path::new("/home/u/.swarmux"));
        assert_eq!(state.why, Why::Existing);
        // Even when the platform folders exist too: the old folder wins.
        let (config, ..) = locate(&no_env, home, Platform::Unix, &|_| true);
        assert_eq!(config.dir, Path::new("/home/u/.swarmux"));
    }

    #[test]
    fn explicit_variables_beat_everything() {
        let home = Path::new("/home/u");
        let env = env_of(&[
            ("SWARMUX_CONFIG_DIR", "/c"),
            ("SWARMUX_STATE_DIR", "/s"),
            ("SWARMUX_MANAGED_FILE", "/m.toml"),
        ]);
        let (config, state, managed) = locate(&env, home, Platform::Unix, &|_| true);
        assert_eq!(
            (config.dir.as_path(), state.dir.as_path()),
            (Path::new("/c"), Path::new("/s"))
        );
        assert_eq!(config.why, Why::Explicit("SWARMUX_CONFIG_DIR"));
        assert_eq!(managed.dir, Path::new("/m.toml"));
        // Config and state are independent.
        let env = env_of(&[("SWARMUX_CONFIG_DIR", "/c")]);
        let (config, state, _) = locate(&env, home, Platform::Unix, &|_| false);
        assert_eq!(config.dir, Path::new("/c"));
        assert_eq!(state.dir, Path::new("/home/u/.local/state/swarmux"));
    }

    #[test]
    fn macos_uses_application_support_and_accepts_dot_config() {
        let home = Path::new("/Users/u");
        let (config, state, _) = locate(&no_env, home, Platform::Mac, &|_| false);
        assert_eq!(
            config.dir,
            Path::new("/Users/u/Library/Application Support/swarmux")
        );
        assert_eq!(
            state.dir,
            Path::new("/Users/u/Library/Application Support/swarmux/state")
        );
        let dot_config = home.join(".config").join("swarmux");
        let (config, ..) = locate(&no_env, home, Platform::Mac, &|p| p == dot_config);
        assert_eq!(config.dir, dot_config, "accepted when it is there");
    }

    #[test]
    fn windows_uses_appdata_and_localappdata() {
        let env = env_of(&[
            ("APPDATA", r"C:\Users\u\AppData\Roaming"),
            ("LOCALAPPDATA", r"C:\Users\u\AppData\Local"),
            ("ProgramData", r"C:\ProgramData"),
        ]);
        let (config, state, managed) =
            locate(&env, Path::new(r"C:\Users\u"), Platform::Windows, &|_| {
                false
            });
        assert_eq!(
            config.dir,
            Path::new(r"C:\Users\u\AppData\Roaming").join("swarmux")
        );
        assert_eq!(
            state.dir,
            Path::new(r"C:\Users\u\AppData\Local").join("swarmux")
        );
        assert_eq!(
            managed.dir,
            Path::new(r"C:\ProgramData")
                .join("swarmux")
                .join("managed.toml")
        );
    }

    #[test]
    fn a_fixture_home_with_an_existing_install_behaves_as_it_always_did() {
        let home = tempfile::tempdir().unwrap();
        let old = home.path().join(".swarmux");
        std::fs::create_dir_all(old.join("holdout")).unwrap();
        std::fs::write(old.join("config.toml"), "max_concurrent = 3\n").unwrap();
        std::fs::write(old.join("swarmux.db"), b"").unwrap();
        std::fs::write(old.join("audit.key"), b"k").unwrap();
        // Whatever the platform and variables, nothing points anywhere else.
        for platform in [Platform::Unix, Platform::Mac, Platform::Windows] {
            let p = Paths::resolve(home.path(), &no_env, platform);
            assert_eq!(p.config_dir, old);
            assert_eq!(p.data_dir, old);
            assert_eq!(p.config, old.join("config.toml"));
            assert_eq!(p.db, old.join("swarmux.db"));
            assert_eq!(p.holdout, old.join("holdout"));
            assert_eq!(p.verify, old.join("verify"));
            assert_eq!(p.config_why, Why::Existing);
        }
    }
}
