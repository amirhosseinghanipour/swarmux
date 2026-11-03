//! `swarmux statusline`: the command Claude Code runs to draw its status
//! line. It hands the payload to swarmux and prints a compact line itself.
//!
//! Inside a swarmux job the payload goes over the hook socket. A session
//! swarmux didn't start has no socket, so the payload is left in a spool
//! folder that [`SpoolFeed`] reads. Whatever goes wrong, the line is still
//! printed and the exit code is 0.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use swarmux_application::StatuslineFeed;
use swarmux_domain::JobId;
use swarmux_domain::telemetry::Statusline;

use super::client::send;
use super::protocol::WireRequest;
use super::transport::{ENV_JOB, ENV_PIPE};
use crate::telemetry::statusline::parse_statusline;

/// Largest payload read from stdin, in bytes.
const INPUT_MAX: u64 = 256 * 1024;
/// The spool folder is looked at no more often than this.
const SCAN_EVERY: std::time::Duration = std::time::Duration::from_secs(1);
/// Spool files older than this are removed.
const SPOOL_KEEP: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Where payloads of sessions without a socket are left.
pub fn spool_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("statusline")
}

/// Runs `swarmux statusline` with this process's stdin, stdout and
/// environment; payloads without a socket go to `spool`.
pub fn statusline_main(spool: Option<&Path>) {
    let mut input = String::new();
    // A payload that can't be read leaves an empty line.
    let _ = std::io::stdin().take(INPUT_MAX).read_to_string(&mut input);
    let pipe = std::env::var(ENV_PIPE).ok();
    let job = std::env::var(ENV_JOB).ok();
    let line = run_statusline(&input, pipe.as_deref(), job.as_deref(), spool, now_ms());
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}

/// Forwards `payload` and returns the line to print.
pub fn run_statusline(
    payload: &str,
    pipe: Option<&str>,
    job: Option<&str>,
    spool: Option<&Path>,
    now: i64,
) -> String {
    let job_id = job.and_then(|j| j.trim().parse::<i64>().ok());
    let Some(parsed) = parse_statusline(payload, job_id.map(JobId), now) else {
        return Statusline::default().compact(now);
    };
    let pipe = pipe.map(str::trim).filter(|p| !p.is_empty());
    let sent = pipe.is_some_and(|p| send(&WireRequest::statusline(payload, job_id), p));
    if !sent && let Some(dir) = spool {
        // Not being able to write is not worth breaking the line over.
        let _ = write_spool(dir, &parsed.session_id, payload);
    }
    parsed.compact(now)
}

fn now_ms() -> i64 {
    system_ms(SystemTime::now())
}

fn system_ms(t: SystemTime) -> i64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// `<spool>/<session>.json`, written whole or not at all.
fn write_spool(dir: &Path, session: &str, payload: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let name: String = session
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(64)
        .collect();
    if name.is_empty() {
        return Ok(());
    }
    let tmp = dir.join(format!("{name}.tmp"));
    std::fs::write(&tmp, payload)?;
    std::fs::rename(&tmp, dir.join(format!("{name}.json")))
}

/// Reads payloads the status line command left in the spool folder.
pub struct SpoolFeed {
    dir: PathBuf,
    state: RefCell<SpoolState>,
}

struct SpoolState {
    seen: HashMap<PathBuf, SystemTime>,
    scanned: Option<Instant>,
}

impl SpoolFeed {
    pub fn new(dir: PathBuf) -> SpoolFeed {
        SpoolFeed {
            dir,
            state: RefCell::new(SpoolState {
                seen: HashMap::new(),
                scanned: None,
            }),
        }
    }
}

impl StatuslineFeed for SpoolFeed {
    fn poll_statuslines(&self) -> Vec<Statusline> {
        let mut state = self.state.borrow_mut();
        if state.scanned.is_some_and(|at| at.elapsed() < SCAN_EVERY) {
            return Vec::new();
        }
        state.scanned = Some(Instant::now());
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            if modified.elapsed().is_ok_and(|age| age > SPOOL_KEEP) {
                // Gone already is fine too.
                let _ = std::fs::remove_file(&path);
                continue;
            }
            if state.seen.get(&path) == Some(&modified) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            state.seen.insert(path, modified);
            out.extend(parse_statusline(&text, None, system_ms(modified)));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_793_613_600_000;
    const PAYLOAD: &str = r#"{"session_id":"s-1","model":{"display_name":"Haiku"},
        "rate_limits":{"five_hour":{"used_percentage":30,"resets_at":1793620000}}}"#;

    #[test]
    fn the_line_is_printed_even_when_nothing_can_be_sent() {
        let line = run_statusline(PAYLOAD, None, None, None, NOW);
        assert_eq!(line, "Haiku · 5h 30%");
        assert_eq!(run_statusline("garbage", None, None, None, NOW), "swarmux");
        let pipe = super::super::transport::endpoint_for(u32::MAX - 2);
        assert_eq!(
            run_statusline(PAYLOAD, Some(&pipe), Some("3"), None, NOW),
            "Haiku · 5h 30%"
        );
    }

    #[test]
    fn without_a_socket_the_payload_is_spooled_and_read_back_once() {
        let dir = tempfile::tempdir().unwrap();
        let spool = dir.path().join("statusline");
        run_statusline(PAYLOAD, None, None, Some(&spool), NOW);
        let feed = SpoolFeed::new(spool.clone());
        let got = feed.poll_statuslines();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].session_id, "s-1");
        assert_eq!(got[0].job, None);
        assert!((got[0].five_hour.unwrap().used - 0.3).abs() < 1e-9);
        // Unchanged files aren't delivered again.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(feed.poll_statuslines().is_empty());
    }

    #[test]
    fn an_unreachable_socket_falls_back_to_the_spool() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = super::super::transport::endpoint_for(u32::MAX - 3);
        run_statusline(PAYLOAD, Some(&pipe), Some("3"), Some(dir.path()), NOW);
        assert!(dir.path().join("s-1.json").exists());
    }

    #[test]
    fn unsafe_session_names_stay_inside_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        write_spool(dir.path(), "../../evil", "{}").unwrap();
        assert!(dir.path().join("evil.json").exists());
        write_spool(dir.path(), "///", "{}").unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
