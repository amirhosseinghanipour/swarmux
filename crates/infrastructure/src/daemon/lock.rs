//! One background service per data folder: a lock file held for as long
//! as the engine runs, and a pid file beside it saying who holds it.
//!
//! The lock is the operating system's, so it goes when the process does:
//! a crash leaves no lock behind, only a pid file, which the next holder
//! overwrites. On Windows the lock file is opened with no sharing, so a
//! second open fails while the first handle lives; elsewhere it is an
//! exclusive `flock`.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const LOCK_FILE: &str = "daemon.lock";
const PID_FILE: &str = "daemon.pid";

/// Who the pid file says holds the lock. Fields it doesn't have are `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Holder {
    pub pid: Option<u32>,
    pub version: Option<String>,
    pub started_at: Option<i64>,
}

#[derive(Debug)]
pub enum LockError {
    /// Another process holds it.
    Held(Holder),
    Io(io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::Held(h) => match h.pid {
                Some(pid) => write!(f, "swarmux is already running here (pid {pid})"),
                None => write!(f, "swarmux is already running here"),
            },
            LockError::Io(e) => write!(f, "cannot take the service lock: {e}"),
        }
    }
}

impl std::error::Error for LockError {}

/// The engine's lock on a data folder, released when dropped.
pub struct ServiceLock {
    _file: File,
    pid_path: PathBuf,
}

impl ServiceLock {
    /// Takes the lock on `dir`, or says who has it. A pid file a crashed
    /// run left is replaced.
    pub fn acquire(dir: &Path, version: &str, now_ms: i64) -> Result<ServiceLock, LockError> {
        std::fs::create_dir_all(dir).map_err(LockError::Io)?;
        let file = match open_locked(&dir.join(LOCK_FILE)) {
            Ok(file) => file,
            Err(e) if is_contended(&e) => return Err(LockError::Held(holder(dir))),
            Err(e) => return Err(LockError::Io(e)),
        };
        let pid_path = dir.join(PID_FILE);
        let text = format!("{}\n{version}\n{now_ms}\n", std::process::id());
        let mut pid = File::create(&pid_path).map_err(LockError::Io)?;
        pid.write_all(text.as_bytes()).map_err(LockError::Io)?;
        Ok(ServiceLock {
            _file: file,
            pid_path,
        })
    }
}

impl Drop for ServiceLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.pid_path);
    }
}

/// Whether some process holds the lock on `dir` now. Takes it for an
/// instant when nobody does, and leaves the pid file alone.
pub fn is_held(dir: &Path) -> bool {
    let path = dir.join(LOCK_FILE);
    // Nothing ever took it: asking leaves no file behind.
    if !path.exists() {
        return false;
    }
    match open_locked(&path) {
        Ok(_) => false,
        Err(e) => is_contended(&e),
    }
}

/// What the pid file in `dir` says, if there is one.
pub fn holder(dir: &Path) -> Holder {
    let Ok(text) = std::fs::read_to_string(dir.join(PID_FILE)) else {
        return Holder::default();
    };
    let mut lines = text.lines();
    Holder {
        pid: lines.next().and_then(|l| l.trim().parse().ok()),
        version: lines
            .next()
            .map(|l| l.trim().to_string())
            .filter(|v| !v.is_empty()),
        started_at: lines.next().and_then(|l| l.trim().parse().ok()),
    }
}

#[cfg(windows)]
fn open_locked(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(path)
}

#[cfg(windows)]
fn is_contended(e: &io::Error) -> bool {
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    matches!(
        e.raw_os_error(),
        Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
    )
}

#[cfg(not(windows))]
fn open_locked(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    // SAFETY: flock on a descriptor this function owns.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

#[cfg(not(windows))]
fn is_contended(e: &io::Error) -> bool {
    e.raw_os_error() == Some(libc::EWOULDBLOCK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_holder_is_refused_and_told_who_holds_it() {
        let dir = tempfile::tempdir().unwrap();
        let first = ServiceLock::acquire(dir.path(), "0.1.0", 1_000).unwrap();
        match ServiceLock::acquire(dir.path(), "0.1.0", 2_000) {
            Err(LockError::Held(h)) => {
                assert_eq!(h.pid, Some(std::process::id()));
                assert_eq!(h.version.as_deref(), Some("0.1.0"));
                assert_eq!(h.started_at, Some(1_000));
            }
            Err(e) => panic!("{e}"),
            Ok(_) => panic!("two holders"),
        }
        assert!(is_held(dir.path()));
        drop(first);
        assert!(!is_held(dir.path()));
        assert_eq!(holder(dir.path()), Holder::default(), "the pid file goes");
        assert!(ServiceLock::acquire(dir.path(), "0.1.0", 3_000).is_ok());
    }

    #[test]
    fn a_crashed_runs_pid_file_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(PID_FILE), "4294967\n0.0.9\n5\n").unwrap();
        std::fs::write(dir.path().join(LOCK_FILE), "").unwrap();
        let lock = ServiceLock::acquire(dir.path(), "0.1.0", 9).unwrap();
        let h = holder(dir.path());
        assert_eq!(h.pid, Some(std::process::id()));
        assert_eq!(h.version.as_deref(), Some("0.1.0"));
        drop(lock);
    }

    #[test]
    fn a_damaged_pid_file_reads_as_unknown() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(PID_FILE), "not a pid\n").unwrap();
        let h = holder(dir.path());
        assert_eq!(h.pid, None);
        assert_eq!(h.version, None);
    }
}
