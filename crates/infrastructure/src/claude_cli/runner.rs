//! `AgentRunner` adapter: runs each job as a `claude -p` child process.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard};

use swarmux_application::ports::{AgentRunner, RunRequest, RunSink, RunnerMsg};
use swarmux_domain::JobId;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::runtime::Handle;
use tokio::task::AbortHandle;

use super::command::{build_args, display, user_message};
use super::parse::parse_line;
use crate::hook::{ENV_JOB, ENV_PIPE, hook_settings};

/// Makes `claude` report `system/session_state_changed` on stdout.
const ENV_SESSION_STATE: &str = "CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS";

/// How much of the end of stderr is kept for error messages.
const STDERR_TAIL: usize = 2048;

/// Running jobs; aborting a job's task drops its child, which kills it.
type Running = Arc<Mutex<HashMap<JobId, AbortHandle>>>;

pub struct ClaudeCli {
    bin: PathBuf,
    env: Vec<(String, String)>,
    guard: Option<GuardHook>,
    rt: Handle,
    running: Running,
}

/// The guard's PreToolUse hook, added to every job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuardHook {
    /// The `swarmux` executable that runs `hook pre-tool`.
    pub exe: PathBuf,
    /// The socket the app listens on.
    pub pipe: String,
    /// Claude Code kills the hook after this long.
    pub timeout_secs: u64,
}

impl GuardHook {
    /// `--settings` and its JSON.
    fn args(&self) -> [String; 2] {
        [
            "--settings".into(),
            hook_settings(&self.exe, self.timeout_secs),
        ]
    }
}

/// The map stays usable even if a task panicked while holding the lock.
fn lock(running: &Running) -> MutexGuard<'_, HashMap<JobId, AbortHandle>> {
    running.lock().unwrap_or_else(|e| e.into_inner())
}

impl ClaudeCli {
    pub fn new(bin: PathBuf, rt: Handle) -> ClaudeCli {
        ClaudeCli {
            bin,
            env: Vec::new(),
            guard: None,
            rt,
            running: Running::default(),
        }
    }

    /// How many jobs are still running.
    pub fn running(&self) -> usize {
        lock(&self.running).len()
    }

    /// Routes every job's tool calls through the guard's hook.
    pub fn with_guard(mut self, guard: GuardHook) -> ClaudeCli {
        self.guard = Some(guard);
        self
    }

    /// Extra environment for every child (used by tests).
    pub fn with_env(mut self, env: Vec<(String, String)>) -> ClaudeCli {
        self.env = env;
        self
    }
}

impl AgentRunner for ClaudeCli {
    fn start(&self, request: RunRequest, sink: RunSink) {
        let bin = self.bin.clone();
        let mut env = self.env.clone();
        let job = request.job_id;
        let mut extra_args = Vec::new();
        // Exact idle / running / requires_action instead of guessing.
        env.push((ENV_SESSION_STATE.into(), "1".into()));
        if let Some(guard) = &self.guard {
            extra_args.extend(guard.args());
            env.push((ENV_PIPE.into(), guard.pipe.clone()));
            env.push((ENV_JOB.into(), job.to_string()));
        }
        let running = self.running.clone();
        // Held until the handle is stored, so a job that ends at once still
        // finds (and removes) its own entry.
        let mut map = lock(&self.running);
        let task = self.rt.spawn(async move {
            run(bin, env, extra_args, request, sink).await;
            lock(&running).remove(&job);
        });
        map.insert(job, task.abort_handle());
    }

    fn cancel(&self, job: JobId) {
        if let Some(task) = lock(&self.running).remove(&job) {
            task.abort();
        }
    }

    fn command_line(&self, request: &RunRequest) -> Option<String> {
        let mut args = build_args(request);
        if let Some(guard) = &self.guard {
            args.extend(guard.args());
        }
        Some(display(&args))
    }
}

async fn run(
    bin: PathBuf,
    env: Vec<(String, String)>,
    extra_args: Vec<String>,
    req: RunRequest,
    sink: RunSink,
) {
    let mut cmd = Command::new(&bin);
    cmd.args(build_args(&req))
        .args(extra_args)
        .envs(env)
        .current_dir(&req.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flash per job

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            sink(RunnerMsg::SpawnFailed(format!("{}: {e}", bin.display())));
            return;
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let msg = user_message(&req.prompt) + "\n";
        // If the child already exited, its exit status explains why.
        let _ = stdin.write_all(msg.as_bytes()).await;
    } // stdin dropped: claude sees EOF after the one message

    let stderr_task = child.stderr.take().map(|mut stderr| {
        tokio::spawn(async move {
            let mut tail = Vec::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                tail.extend_from_slice(&buf[..n]);
                if tail.len() > STDERR_TAIL * 2 {
                    tail.drain(..tail.len() - STDERR_TAIL);
                }
            }
            last_bytes(&String::from_utf8_lossy(&tail), STDERR_TAIL)
        })
    });

    if let Some(stdout) = child.stdout.take() {
        let mut reader = BufReader::new(stdout);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    for ev in parse_line(&String::from_utf8_lossy(&line)) {
                        sink(RunnerMsg::Event(ev));
                    }
                }
            }
        }
    }

    let code = child.wait().await.ok().and_then(|s| s.code());
    let stderr_tail = match stderr_task {
        Some(t) => t.await.unwrap_or_default(),
        None => String::new(),
    };
    sink(RunnerMsg::Exited { code, stderr_tail });
}

fn last_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}
