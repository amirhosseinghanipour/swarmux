//! The background service's log: `daemon.log` in the data folder, moved to
//! `daemon.log.1` once it passes a size cap, so it never grows without end.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The log is moved aside past this size.
pub const LOG_CAP: u64 = 4 << 20;

/// `daemon.log` in `data_dir`.
pub fn log_path(data_dir: &Path) -> PathBuf {
    data_dir.join("daemon.log")
}

/// Moves `path` to `path.1` (replacing an older one) when it is past `cap`.
pub fn rotate(path: &Path, cap: u64) -> io::Result<bool> {
    let big = std::fs::metadata(path).is_ok_and(|m| m.len() > cap);
    if !big {
        return Ok(false);
    }
    let old = rotated(path);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(path, &old)?;
    Ok(true)
}

fn rotated(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".1");
    PathBuf::from(name)
}

/// Lines the service writes, each stamped with the UTC minute.
pub struct ServiceLog {
    path: PathBuf,
    cap: u64,
    file: Mutex<Option<File>>,
}

impl ServiceLog {
    pub fn open(path: &Path, cap: u64) -> ServiceLog {
        let _ = rotate(path, cap);
        ServiceLog {
            path: path.to_path_buf(),
            cap,
            file: Mutex::new(append(path).ok()),
        }
    }

    /// Writes one line. A log that can't be written is not an error the
    /// service stops for.
    pub fn line(&self, now_ms: i64, text: &str) {
        let mut file = self
            .file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if std::fs::metadata(&self.path).is_ok_and(|m| m.len() > self.cap) {
            *file = None;
            if rotate(&self.path, self.cap).is_ok() {
                *file = append(&self.path).ok();
            }
        }
        if file.is_none() {
            *file = append(&self.path).ok();
        }
        if let Some(f) = file.as_mut() {
            let stamp = swarmux_domain::iso_minute(now_ms);
            let _ = writeln!(f, "{stamp} {text}");
        }
    }
}

fn append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_moves_aside_past_its_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = log_path(dir.path());
        let log = ServiceLog::open(&path, 64);
        for i in 0..10 {
            log.line(0, &format!("line {i} of the service log"));
        }
        let now = std::fs::metadata(&path).unwrap().len();
        let old = std::fs::metadata(rotated(&path)).unwrap().len();
        assert!(now <= 64 + 64, "{now}");
        assert!(old > 64, "{old}");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.ends_with("line 9 of the service log\n"), "{text}");
    }

    #[test]
    fn a_small_log_stays() {
        let dir = tempfile::tempdir().unwrap();
        let path = log_path(dir.path());
        std::fs::write(&path, "short\n").unwrap();
        assert!(!rotate(&path, 1024).unwrap());
        assert!(path.is_file());
    }
}
