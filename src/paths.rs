use std::path::{Path, PathBuf};

use anyhow::Context;

/// Where swarmux keeps its data. Injectable so tests never touch the real home.
#[derive(Clone, Debug)]
pub struct Paths {
    pub data_dir: PathBuf,
    pub db: PathBuf,
}

impl Paths {
    pub fn from_home(home: &Path) -> Paths {
        let data_dir = home.join(".swarmux");
        let db = data_dir.join("swarmux.db");
        Paths { data_dir, db }
    }

    pub fn default_home() -> anyhow::Result<Paths> {
        let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        let home = std::env::var_os(var).with_context(|| format!("{var} is not set"))?;
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
    }
}
