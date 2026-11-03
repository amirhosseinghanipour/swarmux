//! Replacing a file that may be the running executable.
//!
//! Windows will not overwrite or delete an executable that is running, but
//! it does let one be renamed. So the new file is written beside the old
//! one, the old one is renamed aside, and the new one is renamed into place;
//! if that last step fails the old one is put back.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use swarmux_application::dist::{BinaryReplacer, DistError, DistResult};

/// Name of the file made by appending `suffix` to `path`'s file name.
pub(crate) fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().map(OsString::from).unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}

/// What a replacement leaves beside the file for a moment.
pub(crate) const STAGED: &str = ".new";
pub(crate) const ASIDE: &str = ".old";

/// Puts `bytes` at `dest`, replacing what is there, safely even on Windows
/// when `dest` is running.
pub fn replace_file(dest: &Path, bytes: &[u8]) -> io::Result<()> {
    let staged = sibling(dest, STAGED);
    let aside = sibling(dest, ASIDE);
    // Leftovers of an earlier replacement.
    let _ = std::fs::remove_file(&aside);

    std::fs::write(&staged, bytes)?;
    make_executable(&staged)?;

    let had_old = dest.exists();
    if had_old && let Err(e) = std::fs::rename(dest, &aside) {
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&staged, dest) {
        if had_old {
            let _ = std::fs::rename(&aside, dest);
        }
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }
    // A running executable cannot be deleted on Windows; the next
    // replacement or `uninstall` takes it away.
    let _ = std::fs::remove_file(&aside);
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Replaces one executable (normally the running one).
pub struct ExeReplacer {
    exe: PathBuf,
}

impl ExeReplacer {
    pub fn new(exe: PathBuf) -> Self {
        Self { exe }
    }
}

impl BinaryReplacer for ExeReplacer {
    fn replace(&self, new_binary: &[u8]) -> DistResult<()> {
        if new_binary.is_empty() {
            return Err(DistError::Archive("the new executable is empty".into()));
        }
        replace_file(&self.exe, new_binary)
            .map_err(|e| DistError::Io(format!("cannot replace {}: {e}", self.exe.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_and_leaves_nothing_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("swarmux.exe");
        std::fs::write(&exe, b"old").unwrap();
        replace_file(&exe, b"new").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![OsString::from("swarmux.exe")]);
    }

    #[test]
    fn creates_when_missing_and_refuses_an_empty_binary() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("swarmux");
        replace_file(&exe, b"x").unwrap();
        assert!(exe.exists());
        assert!(ExeReplacer::new(exe.clone()).replace(b"").is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"x");
    }
}
