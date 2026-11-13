//! The spool of hook calls that fell back because swarmux could not be
//! reached.
//!
//! The hook process appends one JSON line per fallback to a file in the
//! state folder; the app reads the file at its next start, writes the lines
//! into the ledger and empties it. The hook only ever appends, so a hook
//! that gives up while the app is down loses nothing.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use swarmux_application::ports::{FallbackRecord, FallbackSpool};
use swarmux_application::{AppError, AppResult};
use swarmux_domain::JobId;
use swarmux_domain::ledger::{hex, sha256};

/// The environment variable naming the spool file, set on every job.
pub const ENV_SPOOL: &str = "SWARMUX_GUARD_SPOOL";
/// The environment variable with what the hook does when it can't reach
/// the app: `closed`, `ask` or `open`.
pub const ENV_FAIL: &str = "SWARMUX_GUARD_FAIL";

/// The spool file of the state folder `data_dir`.
pub fn spool_file(data_dir: &Path) -> PathBuf {
    data_dir.join("guard-fallback.jsonl")
}

/// One spooled line.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Line {
    ts: i64,
    #[serde(default)]
    job: Option<i64>,
    #[serde(default)]
    tool: String,
    #[serde(default)]
    input_sha256: String,
    reason: String,
    mode: String,
}

/// Appends a fallback to `spool`. Best effort: a hook that can't write the
/// spool still answers.
pub fn append(spool: &Path, record: &FallbackRecord) -> bool {
    let line = Line {
        ts: record.ts,
        job: record.job.map(|j| j.0),
        tool: record.tool.clone(),
        input_sha256: record.input_sha256.clone(),
        reason: record.reason.clone(),
        mode: record.mode.clone(),
    };
    let Ok(mut text) = serde_json::to_string(&line) else {
        return false;
    };
    text.push('\n');
    if let Some(dir) = spool.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(spool)
        // One write, so lines from hooks running at once don't interleave.
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .is_ok()
}

/// `FallbackSpool` over the spool file.
pub struct FileSpool {
    path: PathBuf,
}

impl FileSpool {
    pub fn new(path: PathBuf) -> FileSpool {
        FileSpool { path }
    }
}

impl FallbackSpool for FileSpool {
    fn read(&self) -> AppResult<Vec<FallbackRecord>> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(AppError::Storage(format!("{}: {e}", self.path.display()))),
        };
        // A line that doesn't parse is kept as a record of its own, not dropped.
        Ok(text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|raw| {
                let line_id = hex(&sha256(raw.as_bytes()))[..16].to_string();
                match serde_json::from_str::<Line>(raw) {
                    Ok(l) => FallbackRecord {
                        ts: l.ts,
                        job: l.job.map(JobId),
                        tool: l.tool,
                        input_sha256: l.input_sha256,
                        reason: l.reason,
                        mode: l.mode,
                        line_id,
                    },
                    Err(_) => FallbackRecord {
                        ts: 0,
                        job: None,
                        tool: String::new(),
                        input_sha256: String::new(),
                        reason: "unreadable spool line".into(),
                        mode: "unknown".into(),
                        line_id,
                    },
                }
            })
            .collect())
    }

    fn clear(&self) -> AppResult<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(AppError::Storage(format!("{}: {e}", self.path.display()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(reason: &str, mode: &str) -> FallbackRecord {
        FallbackRecord {
            ts: 1_700_000_000_000,
            job: Some(JobId(7)),
            tool: "Bash".into(),
            input_sha256: "ab".repeat(32),
            reason: reason.into(),
            mode: mode.into(),
            line_id: String::new(),
        }
    }

    #[test]
    fn appended_lines_are_read_back_and_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let path = spool_file(dir.path());
        let spool = FileSpool::new(path.clone());
        assert_eq!(spool.read().unwrap(), []);
        assert!(append(&path, &record("connect failed", "deny")));
        assert!(append(&path, &record("no answer", "pass")));
        let read = spool.read().unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(
            (read[0].reason.as_str(), read[0].mode.as_str()),
            ("connect failed", "deny")
        );
        assert_eq!(read[0].job, Some(JobId(7)));
        assert_ne!(read[0].line_id, read[1].line_id);
        spool.clear().unwrap();
        assert_eq!(spool.read().unwrap(), []);
        spool.clear().unwrap();
    }

    #[test]
    fn a_damaged_line_is_still_a_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = spool_file(dir.path());
        std::fs::write(&path, "not json\n").unwrap();
        let read = FileSpool::new(path).read().unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].reason, "unreadable spool line");
    }
}
