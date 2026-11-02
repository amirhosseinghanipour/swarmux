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
}

impl Paths {
    pub fn from_home(home: &Path) -> Paths {
        let data_dir = home.join(".swarmux");
        let db = data_dir.join("swarmux.db");
        let config = data_dir.join("config.toml");
        Paths {
            home: home.to_path_buf(),
            data_dir,
            db,
            config,
        }
    }

    pub fn default_home() -> io::Result<Paths> {
        let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        let home = std::env::var_os(var)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{var} is not set")))?;
        Ok(Paths::from_home(Path::new(&home)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_lives_under_dot_swarmux() {
        let p = Paths::from_home(Path::new("/home/u"));
        assert_eq!(p.data_dir, Path::new("/home/u/.swarmux"));
        assert_eq!(p.db, Path::new("/home/u/.swarmux/swarmux.db"));
        assert_eq!(p.config, Path::new("/home/u/.swarmux/config.toml"));
    }
}
