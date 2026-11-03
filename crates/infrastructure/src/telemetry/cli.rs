//! Runs a CLI (git, gh) with a time limit: no shell, no console window. At
//! the limit the process tree is killed.

use std::io::{ErrorKind, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::process::spawn_std;

/// Why a command gave no output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliError {
    /// The program isn't installed (or not on PATH).
    NotFound,
    /// It exited unsuccessfully; the first line of its stderr.
    Failed(String),
    TimedOut,
    Io(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::NotFound => write!(f, "not installed"),
            CliError::Failed(msg) => write!(f, "{msg}"),
            CliError::TimedOut => write!(f, "timed out"),
            CliError::Io(msg) => write!(f, "{msg}"),
        }
    }
}

/// Runs `bin args…` in `cwd` and returns its stdout, or why not.
pub fn run(
    bin: &Path,
    args: &[&str],
    cwd: Option<&Path>,
    limit: Duration,
) -> Result<String, CliError> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // No pager, no prompts.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1");
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let (mut child, tree) = spawn_std(&mut cmd).map_err(|e| match e.kind() {
        ErrorKind::NotFound => CliError::NotFound,
        _ => CliError::Io(e.to_string()),
    })?;
    // Read both pipes on their own threads so a full pipe never stalls the child.
    let out = child.stdout.take().map(drain);
    let err = child.stderr.take().map(drain);
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= limit => {
                // Already gone is fine too.
                tree.kill();
                let _ = child.wait();
                return Err(CliError::TimedOut);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err(CliError::Io(e.to_string())),
        }
    };
    let stdout = out.and_then(|h| h.join().ok()).unwrap_or_default();
    let stderr = err.and_then(|h| h.join().ok()).unwrap_or_default();
    if status.success() {
        Ok(stdout)
    } else {
        let first = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        Err(CliError::Failed(first.trim().to_string()))
    }
}

fn drain(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        // A read error leaves what was read so far.
        let _ = pipe.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_program_is_not_found() {
        let r = run(
            Path::new("surely-no-such-program-xyz"),
            &[],
            None,
            Duration::from_secs(5),
        );
        assert_eq!(r, Err(CliError::NotFound));
    }

    #[test]
    fn output_and_failure_of_git() {
        let git = Path::new("git");
        match run(git, &["--version"], None, Duration::from_secs(10)) {
            Ok(out) => assert!(out.starts_with("git version"), "{out}"),
            Err(CliError::NotFound) => return eprintln!("git not available; skipping"),
            Err(e) => panic!("{e}"),
        }
        let dir = tempfile::tempdir().unwrap();
        let r = run(git, &["status"], Some(dir.path()), Duration::from_secs(10));
        assert!(
            matches!(r, Err(CliError::Failed(ref m)) if m.contains("not a git repository")),
            "{r:?}"
        );
    }
}
