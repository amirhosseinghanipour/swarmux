//! `Checker` adapter: runs one gate check as a child process.
//!
//! The program is started directly with its arguments, never through a
//! shell, with no console window on Windows. Its output is kept within
//! [`OUTPUT_MAX`] bytes (the start and the end), and at its timeout the
//! whole process tree is killed (a Job Object on Windows).

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use swarmux_application::Checker;
use swarmux_domain::gate::{CheckRun, GateCheck, env_passes};

use crate::process::spawn_std;

/// Output kept per run, in bytes.
pub const OUTPUT_MAX: usize = 64 * 1024;
/// Of that, the start of the output kept.
const HEAD_MAX: usize = 8 * 1024;
/// How often a running check is looked at.
const POLL: Duration = Duration::from_millis(20);
/// How long the output is waited for once the process has gone.
const DRAIN: Duration = Duration::from_secs(2);

/// Runs checks as child processes.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessChecker;

impl ProcessChecker {
    pub fn new() -> ProcessChecker {
        ProcessChecker
    }
}

/// The start and the end of a stream of bytes.
#[derive(Default)]
struct Kept {
    head: Vec<u8>,
    tail: Vec<u8>,
    dropped: usize,
}

impl Kept {
    fn push(&mut self, mut bytes: &[u8]) {
        if self.head.len() < HEAD_MAX {
            let n = (HEAD_MAX - self.head.len()).min(bytes.len());
            self.head.extend_from_slice(&bytes[..n]);
            bytes = &bytes[n..];
        }
        self.tail.extend_from_slice(bytes);
        let limit = OUTPUT_MAX - HEAD_MAX;
        if self.tail.len() > 2 * limit {
            let cut = self.tail.len() - limit;
            self.dropped += cut;
            self.tail.drain(..cut);
        }
    }

    fn text(&self) -> String {
        let limit = OUTPUT_MAX - HEAD_MAX;
        let (cut, tail) = if self.tail.len() > limit {
            let cut = self.tail.len() - limit;
            (self.dropped + cut, &self.tail[cut..])
        } else {
            (self.dropped, &self.tail[..])
        };
        let mut out = String::from_utf8_lossy(&self.head).into_owned();
        if cut > 0 {
            out.push_str(&format!("\n… {cut} bytes left out …\n"));
        }
        out.push_str(&String::from_utf8_lossy(tail));
        out
    }
}

impl Checker for ProcessChecker {
    fn run(&self, check: &GateCheck, root: &Path) -> CheckRun {
        let started = Instant::now();
        let mut run = CheckRun {
            name: check.name.clone(),
            command: check.command_line(),
            exit: None,
            timed_out: false,
            spawn_error: None,
            duration_ms: 0,
            output: String::new(),
            flaky: false,
        };
        let dir = check.workdir(root);
        if !dir.is_dir() {
            run.spawn_error = Some(format!("{} is not a folder", dir.display()));
            return run;
        }
        let Some(program) = resolve(&check.program, &dir) else {
            run.spawn_error = Some(format!("{} was not found", check.program));
            return run;
        };
        let mut cmd = Command::new(&program);
        // A check sees what a program needs to find its tools and its
        // home, and what the check lists: never swarmux's own variables or
        // a provider's credentials, which the agent's code runs next to.
        cmd.env_clear();
        for (name, value) in scrubbed_env(std::env::vars_os(), &check.env) {
            cmd.env(name, value);
        }
        cmd.args(&check.args)
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("NO_COLOR", "1")
            .env("CARGO_TERM_COLOR", "never");
        let (mut child, tree) = match spawn_std(&mut cmd) {
            Ok(started) => started,
            Err(e) => {
                run.spawn_error = Some(e.to_string());
                return run;
            }
        };
        let kept = Arc::new(Mutex::new(Kept::default()));
        let (done_tx, done_rx) = channel::<()>();
        let mut readers = 0;
        let pipes: [Option<Box<dyn Read + Send>>; 2] = [
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn Read + Send>),
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn Read + Send>),
        ];
        for mut pipe in pipes.into_iter().flatten() {
            readers += 1;
            let kept = kept.clone();
            let done = done_tx.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                while let Ok(n) = pipe.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    kept.lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(&buf[..n]);
                }
                let _ = done.send(());
            });
        }
        drop(done_tx);

        let deadline = started + Duration::from_secs(check.timeout_secs);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() >= deadline => {
                    run.timed_out = true;
                    tree.kill();
                    break child.wait().ok();
                }
                Ok(None) => std::thread::sleep(POLL),
                Err(_) => {
                    tree.kill();
                    break child.wait().ok();
                }
            }
        };
        // Grandchildren may hold the pipes open; don't wait for them long.
        let drain_until = Instant::now() + DRAIN;
        for _ in 0..readers {
            let left = drain_until.saturating_duration_since(Instant::now());
            if done_rx.recv_timeout(left).is_err() {
                break;
            }
        }
        run.exit = status.and_then(|s| s.code()).filter(|_| !run.timed_out);
        run.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        run.output = kept.lock().unwrap_or_else(PoisonError::into_inner).text();
        run
    }
}

/// The variables of `vars` a check that lists `extra` may see.
pub(crate) fn scrubbed_env(
    vars: impl Iterator<Item = (OsString, OsString)>,
    extra: &[String],
) -> Vec<(OsString, OsString)> {
    vars.filter(|(name, _)| name.to_str().is_some_and(|n| env_passes(n, extra)))
        .collect()
}

/// The executable for `program`: a path as given (relative to `root`), or
/// the first match on `PATH`. On Windows `npm` finds `npm.cmd`.
fn resolve(program: &str, root: &Path) -> Option<PathBuf> {
    let given = Path::new(program);
    if given.components().count() > 1 || given.is_absolute() {
        let path = if given.is_absolute() {
            given.to_path_buf()
        } else {
            root.join(given)
        };
        return path.is_file().then_some(path);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let exts: Vec<OsString> = if cfg!(windows) && given.extension().is_none() {
        [".exe", ".com", ".cmd", ".bat"]
            .iter()
            .map(OsString::from)
            .collect()
    } else {
        vec![OsString::new()]
    };
    std::env::split_paths(&path).find_map(|dir| {
        exts.iter().find_map(|ext| {
            let mut name = OsString::from(program);
            name.push(ext);
            let candidate = dir.join(name);
            candidate.is_file().then_some(candidate)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_output_has_the_start_and_the_end() {
        let mut k = Kept::default();
        k.push(b"first line\n");
        for _ in 0..20_000 {
            k.push(b"0123456789");
        }
        k.push(b"\nlast line");
        let text = k.text();
        assert!(text.starts_with("first line\n"));
        assert!(text.ends_with("\nlast line"));
        assert!(text.contains("bytes left out"));
        assert!(text.len() <= OUTPUT_MAX + 64);
        let mut small = Kept::default();
        small.push(b"ok");
        assert_eq!(small.text(), "ok");
    }

    #[test]
    fn a_check_sees_its_tools_and_not_the_secrets_around_it() {
        let vars = [
            ("PATH", "/bin"),
            ("HOME", "/home/me"),
            ("LC_ALL", "C"),
            ("CARGO_HOME", "/c"),
            ("SWARMUX_JOB_ID", "7"),
            ("SWARMUX_HOOK_PIPE", "/p"),
            ("ANTHROPIC_API_KEY", "k"),
            ("OPENAI_API_KEY", "k"),
            ("AWS_SECRET_ACCESS_KEY", "k"),
            ("GITHUB_TOKEN", "t"),
            ("NPM_TOKEN", "t"),
            ("MY_APP_TOKEN", "t"),
            ("DATABASE_URL", "postgres://x"),
            ("RUST_LOG", "debug"),
        ]
        .map(|(k, v)| (OsString::from(k), OsString::from(v)));
        let seen = |extra: &[String]| -> Vec<String> {
            scrubbed_env(vars.clone().into_iter(), extra)
                .into_iter()
                .map(|(k, _)| k.to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(seen(&[]), ["PATH", "HOME", "LC_ALL", "CARGO_HOME"]);
        // A check can ask for more by name, but never for a credential.
        let extra = vec![
            "DATABASE_URL".to_string(),
            "RUST_LOG".to_string(),
            "ANTHROPIC_API_KEY".to_string(),
            "SWARMUX_JOB_ID".to_string(),
            "MY_APP_TOKEN".to_string(),
        ];
        assert_eq!(
            seen(&extra),
            [
                "PATH",
                "HOME",
                "LC_ALL",
                "CARGO_HOME",
                "DATABASE_URL",
                "RUST_LOG"
            ]
        );
    }

    #[test]
    fn a_missing_program_is_reported_not_run() {
        let check = GateCheck::new("x", "surely-not-a-real-program-4711", &[]);
        let run = ProcessChecker.run(&check, Path::new("."));
        assert!(!run.passed());
        assert!(run.spawn_error.unwrap().contains("was not found"));
        let rel = GateCheck::new("x", "./nope/missing", &[]);
        assert!(
            ProcessChecker
                .run(&rel, Path::new("."))
                .spawn_error
                .is_some()
        );
    }

    #[test]
    fn exit_codes_and_output_come_back() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "41\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "42\n").unwrap();
        let diff = |a: &str, b: &str| {
            GateCheck::new("answer", "git", &["diff", "--no-index", "--no-color", a, b])
        };
        let failed = ProcessChecker.run(&diff("a.txt", "b.txt"), dir.path());
        assert_eq!(failed.exit, Some(1));
        assert!(failed.output.contains("-41"), "{}", failed.output);
        assert!(failed.output.contains("+42"));
        let same = ProcessChecker.run(&diff("b.txt", "b.txt"), dir.path());
        assert!(same.passed(), "{same:?}");
    }

    #[test]
    fn a_check_past_its_timeout_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        let mut check = if cfg!(windows) {
            GateCheck::new("wait", "ping", &["-n", "30", "127.0.0.1"])
        } else {
            GateCheck::new("wait", "sleep", &["30"])
        };
        check.timeout_secs = 1;
        let started = Instant::now();
        let run = ProcessChecker.run(&check, dir.path());
        assert!(run.timed_out, "{run:?}");
        assert!(!run.passed());
        assert_eq!(run.exit, None);
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
