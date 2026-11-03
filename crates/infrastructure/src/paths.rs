use std::io;
use std::path::{Path, PathBuf};

/// Where swarmux keeps its data. Injectable so tests never touch the real home.
#[derive(Clone, Debug)]
pub struct Paths {
    /// The user's home folder.
    pub home: PathBuf,
    pub data_dir: PathBuf,
    pub db: PathBuf,
    pub config: PathBuf,
    /// Hidden checks, per repository: never inside a repository.
    pub holdout: PathBuf,
    /// Clean copies the verifier makes and their shared build folders.
    pub verify: PathBuf,
}

impl Paths {
    pub fn from_home(home: &Path) -> Paths {
        let data_dir = home.join(".swarmux");
        let db = data_dir.join("swarmux.db");
        let config = data_dir.join("config.toml");
        let holdout = data_dir.join("holdout");
        let verify = data_dir.join("verify");
        Paths {
            home: home.to_path_buf(),
            data_dir,
            db,
            config,
            holdout,
            verify,
        }
    }

    pub fn default_home() -> io::Result<Paths> {
        let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        let home = std::env::var_os(var)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{var} is not set")))?;
        Ok(Paths::from_home(Path::new(&home)))
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
}
