//! The real machine under `swarmux install` and `swarmux uninstall`.

use std::io;
use std::path::{Path, PathBuf};

use swarmux_application::dist::{DistError, DistResult, InstallHost};
use swarmux_domain::dist::Manifest;

use super::pathenv;
use super::replace::{ASIDE, STAGED, replace_file, sibling};

const MANIFEST_FILE: &str = "install.json";

/// Files and the user's `PATH`, under a given home folder.
pub struct FsInstallHost {
    home: PathBuf,
    /// Whether the user's `PATH` may be read and changed (tests turn it off
    /// so they never touch the real one).
    path_enabled: bool,
}

impl FsInstallHost {
    pub fn new(home: PathBuf) -> Self {
        Self {
            home,
            path_enabled: true,
        }
    }

    /// Leaves the user's `PATH` alone, whatever is asked.
    pub fn without_path(mut self) -> Self {
        self.path_enabled = false;
        self
    }

    fn manifest_path(&self) -> PathBuf {
        self.data_dir().join(MANIFEST_FILE)
    }
}

/// Where the executable goes by default: under `%LOCALAPPDATA%` on Windows,
/// `~/.local/bin` elsewhere.
pub fn default_bin_dir(home: &Path, local_app_data: Option<&Path>) -> PathBuf {
    match local_app_data {
        Some(base) if cfg!(windows) => base.join("Programs").join("swarmux").join("bin"),
        _ => home.join(".local").join("bin"),
    }
}

fn io_err(what: &str, path: &Path, e: &io::Error) -> DistError {
    DistError::Io(format!("cannot {what} {}: {e}", path.display()))
}

impl InstallHost for FsInstallHost {
    fn current_exe(&self) -> DistResult<PathBuf> {
        std::env::current_exe()
            .map_err(|e| DistError::Io(format!("cannot find the swarmux executable: {e}")))
    }

    fn data_dir(&self) -> PathBuf {
        self.home.join(".swarmux")
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn create_dir_all(&self, path: &Path) -> DistResult<()> {
        std::fs::create_dir_all(path).map_err(|e| io_err("create", path, &e))
    }

    fn install_file(&self, from: &Path, to: &Path) -> DistResult<()> {
        let bytes = std::fs::read(from).map_err(|e| io_err("read", from, &e))?;
        replace_file(to, &bytes).map_err(|e| io_err("write", to, &e))
    }

    fn remove_file(&self, path: &Path) -> DistResult<Option<String>> {
        // Leftovers of an update.
        let _ = std::fs::remove_file(sibling(path, ASIDE));
        let _ = std::fs::remove_file(sibling(path, STAGED));
        match std::fs::remove_file(path) {
            Ok(()) => Ok(None),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(first) => {
                // A running program cannot be deleted on Windows but can be
                // moved: out of the folder, into the temp folder.
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                let away = std::env::temp_dir()
                    .join(format!("swarmux-removed-{}-{name}", std::process::id()));
                std::fs::rename(path, &away).map_err(|_| io_err("remove", path, &first))?;
                if std::fs::remove_file(&away).is_ok() {
                    return Ok(None);
                }
                Ok(Some(match schedule_removal(&away) {
                    true => format!(
                        "{} is still running; it will be deleted when Windows restarts",
                        away.display()
                    ),
                    false => format!(
                        "{} is still running; delete it once swarmux has exited",
                        away.display()
                    ),
                }))
            }
        }
    }

    fn remove_dir_if_empty(&self, path: &Path) -> DistResult<bool> {
        match std::fs::remove_dir(path) {
            Ok(()) => Ok(true),
            // Not empty (or already gone): leave it.
            Err(_) => Ok(false),
        }
    }

    fn remove_dir_all(&self, path: &Path) -> DistResult<()> {
        std::fs::remove_dir_all(path).map_err(|e| io_err("remove", path, &e))
    }

    fn path_has(&self, dir: &Path) -> bool {
        self.path_enabled && pathenv::has(dir)
    }

    fn path_add(&self, dir: &Path) -> DistResult<bool> {
        if self.path_enabled {
            pathenv::add(dir)
        } else {
            Ok(false)
        }
    }

    fn path_remove(&self, dir: &Path) -> DistResult<bool> {
        if self.path_enabled {
            pathenv::remove(dir)
        } else {
            Ok(false)
        }
    }

    fn read_manifest(&self) -> DistResult<Option<Manifest>> {
        let path = self.manifest_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_err("read", &path, &e)),
        };
        let json: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| DistError::Io(format!("{} is unreadable: {e}", path.display())))?;
        let text_of = |k: &str| json.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let Some(binary) = text_of("binary") else {
            return Err(DistError::Io(format!("{} has no binary", path.display())));
        };
        Ok(Some(Manifest {
            binary: PathBuf::from(binary),
            created_root: text_of("created_root").map(PathBuf::from),
            path_entry: text_of("path_entry"),
            data_dir_created: json
                .get("data_dir_created")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        }))
    }

    fn write_manifest(&self, m: &Manifest) -> DistResult<()> {
        let path = self.manifest_path();
        let json = serde_json::json!({
            "binary": m.binary.display().to_string(),
            "created_root": m.created_root.as_ref().map(|p| p.display().to_string()),
            "path_entry": m.path_entry,
            "data_dir_created": m.data_dir_created,
        });
        std::fs::write(&path, json.to_string()).map_err(|e| io_err("write", &path, &e))
    }

    fn remove_manifest(&self) -> DistResult<()> {
        let path = self.manifest_path();
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err("remove", &path, &e)),
        }
    }
}

/// Asks Windows to delete `path` at the next restart.
#[cfg(windows)]
fn schedule_removal(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW};
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: a valid NUL-terminated path and a null destination, which
    // means "delete".
    unsafe { MoveFileExW(wide.as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT) != 0 }
}

#[cfg(not(windows))]
fn schedule_removal(_path: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_application::dist::{InstallOptions, InstallService};

    /// A copy of a file to install from, so the "current executable" is
    /// not the test binary itself. The host's current_exe is the test
    /// binary; installing copies it, and uninstall removes the copy.
    fn opts(dir: PathBuf) -> InstallOptions {
        InstallOptions {
            dir,
            edit_path: false,
        }
    }

    #[test]
    fn install_then_uninstall_leaves_no_trace() {
        let home = tempfile::tempdir().unwrap();
        let svc = InstallService::new(FsInstallHost::new(home.path().to_path_buf()).without_path());
        let dir = home.path().join("deep").join("er").join("bin");

        let r = svc.install(&opts(dir.clone())).unwrap();
        assert!(r.done.iter().any(|l| l.starts_with("installed")));
        assert!(dir.is_dir());
        assert!(home.path().join(".swarmux").join(MANIFEST_FILE).is_file());

        let r = svc.uninstall(false).unwrap();
        assert!(r.done.iter().any(|l| l.starts_with("removed")));
        let left: Vec<_> = std::fs::read_dir(home.path()).unwrap().collect();
        assert!(left.is_empty(), "left behind: {left:?}");
    }

    #[test]
    fn user_data_survives_uninstall_unless_purged() {
        let home = tempfile::tempdir().unwrap();
        let svc = InstallService::new(FsInstallHost::new(home.path().to_path_buf()).without_path());
        let dir = home.path().join("bin");
        svc.install(&opts(dir.clone())).unwrap();
        let db = home.path().join(".swarmux").join("swarmux.db");
        std::fs::write(&db, b"data").unwrap();

        let r = svc.uninstall(false).unwrap();
        assert!(db.is_file());
        assert!(!dir.exists());
        assert!(r.notes.iter().any(|n| n.starts_with("kept")));

        // Nothing left to uninstall: said, not failed.
        let again = svc.uninstall(false).unwrap();
        assert!(again.done.is_empty());

        svc.install(&opts(dir)).unwrap();
        svc.uninstall(true).unwrap();
        assert!(!home.path().join(".swarmux").exists());
    }

    #[test]
    fn existing_folders_are_not_taken_back() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("bin");
        std::fs::create_dir(&dir).unwrap();
        let svc = InstallService::new(FsInstallHost::new(home.path().to_path_buf()).without_path());
        svc.install(&opts(dir.clone())).unwrap();
        svc.uninstall(false).unwrap();
        assert!(dir.is_dir(), "a folder that was there before stays");
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none());
    }

    #[test]
    fn the_default_folder_is_per_user() {
        let home = Path::new("/h");
        assert_eq!(
            default_bin_dir(home, None),
            PathBuf::from("/h").join(".local").join("bin")
        );
        if cfg!(windows) {
            assert!(
                default_bin_dir(home, Some(Path::new("/l")))
                    .ends_with(Path::new("Programs").join("swarmux").join("bin"))
            );
        }
    }
}
