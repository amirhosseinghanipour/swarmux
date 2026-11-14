//! `AgentRunner` adapter: runs each job as a `claude -p` child process.
//!
//! Starts are spaced apart, a start that fails because another session was
//! writing Claude Code's config is repeated, and cancelling a job kills the
//! whole process tree.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use swarmux_application::ports::{AgentRunner, RunRequest, RunSink, RunnerMsg};
use swarmux_domain::JobId;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::runtime::Handle;
use tokio::task::AbortHandle;

use super::command::{build_args_with, display, env_for, user_message};
use super::contention::{self, is_config_contention};
use super::parse::parse_line;
use super::runtime::launch::LaunchSpec;
use crate::hook::{ENV_JOB, ENV_PIPE, ENV_SPOOL};
use crate::process::{Stagger, random_below, spawn_tokio};

/// Makes `claude` report `system/session_state_changed` on stdout.
const ENV_SESSION_STATE: &str = "CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS";

/// How much of the end of stderr is kept for error messages.
const STDERR_TAIL: usize = 2048;

/// Running jobs; aborting a job's task drops its process tree, which kills it.
type Running = Arc<Mutex<HashMap<JobId, AbortHandle>>>;

pub struct ClaudeCli {
    bin: PathBuf,
    env: Vec<(String, String)>,
    /// The guard's hook, the Board's server and the profile settings that
    /// shape every launch.
    spec: LaunchSpec,
    rt: Handle,
    running: Running,
    stagger: Arc<Stagger>,
    /// Where each job's hook leaves what it could not ask the app.
    spool: Option<PathBuf>,
}

/// The Board's MCP server, added to the jobs on a mission's board.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardMcp {
    /// The `swarmux` executable that runs `board-mcp`.
    pub exe: PathBuf,
    /// The socket the app listens on.
    pub pipe: String,
}

impl BoardMcp {
    /// `--mcp-config` and `--allowedTools` for `job`.
    pub(super) fn args(&self, job: JobId) -> [String; 4] {
        [
            "--mcp-config".into(),
            self.config_json(job),
            "--allowedTools".into(),
            crate::board::SERVER_NAME_PREFIX.into(),
        ]
    }

    /// The MCP config registering the Board's server for `job`.
    pub(super) fn config_json(&self, job: JobId) -> String {
        crate::board::mcp_config(&self.exe, &self.pipe, job.0)
    }
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

/// The map stays usable even if a task panicked while holding the lock.
fn lock(running: &Running) -> MutexGuard<'_, HashMap<JobId, AbortHandle>> {
    running.lock().unwrap_or_else(|e| e.into_inner())
}

impl ClaudeCli {
    pub fn new(bin: PathBuf, rt: Handle) -> ClaudeCli {
        ClaudeCli {
            bin,
            env: Vec::new(),
            spec: LaunchSpec::default(),
            rt,
            running: Running::default(),
            stagger: Stagger::shared(),
            spool: None,
        }
    }

    /// Where each job's hook writes the calls it could not ask the app
    /// about (`SWARMUX_GUARD_SPOOL`).
    pub fn with_guard_spool(mut self, spool: PathBuf) -> ClaudeCli {
        self.spool = Some(spool);
        self
    }

    /// Adds `onFailure: "block"` to the PreToolUse hook of every job.
    pub fn with_on_failure_block(mut self, on: bool) -> ClaudeCli {
        self.spec.on_failure_block = on;
        self
    }

    /// Launches with `spec`: the guard, the Board and the profile settings
    /// the runtime was given too.
    pub fn with_spec(mut self, spec: LaunchSpec) -> ClaudeCli {
        self.spec = spec;
        self
    }

    /// How many jobs are still running.
    pub fn running(&self) -> usize {
        lock(&self.running).len()
    }

    /// Routes every job's tool calls through the guard's hook.
    pub fn with_guard(mut self, guard: GuardHook) -> ClaudeCli {
        self.spec.guard = Some(guard);
        self
    }

    /// Registers the board's MCP server for the jobs that are on a board.
    pub fn with_board(mut self, board: BoardMcp) -> ClaudeCli {
        self.spec.board = Some(board);
        self
    }

    /// Spaces this runner's starts with `stagger` instead of the shared one.
    pub fn with_stagger(mut self, stagger: Arc<Stagger>) -> ClaudeCli {
        self.stagger = stagger;
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
        // Exact idle / running / requires_action instead of guessing.
        env.push((ENV_SESSION_STATE.into(), "1".into()));
        // A workspace job's repositories load their own instructions.
        env.extend(env_for(&request));
        let extra_args = self.spec.extra_args(&request);
        if let Some(guard) = &self.spec.guard {
            env.push((ENV_PIPE.into(), guard.pipe.clone()));
            env.push((ENV_JOB.into(), job.to_string()));
            if let Some(spool) = &self.spool {
                env.push((ENV_SPOOL.into(), spool.to_string_lossy().into_owned()));
            }
        }
        let launch = self.spec.config.clone();
        let running = self.running.clone();
        let stagger = self.stagger.clone();
        // Held until the handle is stored, so a job that ends at once still
        // finds (and removes) its own entry.
        let mut map = lock(&self.running);
        let task = self.rt.spawn(async move {
            run(bin, env, extra_args, launch, request, sink, stagger).await;
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
        let mut args = build_args_with(request, &self.spec.config);
        args.extend(self.spec.extra_args(request));
        Some(display(&args))
    }
}

/// How one start of `claude` ended.
enum Attempt {
    SpawnFailed(String),
    Exited {
        code: Option<i32>,
        stderr_tail: String,
        /// Whether anything at all came out on stdout.
        produced: bool,
    },
}

async fn run(
    bin: PathBuf,
    env: Vec<(String, String)>,
    extra_args: Vec<String>,
    launch: super::runtime::launch::LaunchConfig,
    req: RunRequest,
    mut sink: RunSink,
    stagger: Arc<Stagger>,
) {
    let mut cmd = Command::new(&bin);
    cmd.args(build_args_with(&req, &launch))
        .args(extra_args)
        .envs(env)
        .envs(req.env.iter().map(|(k, v)| (k, v)))
        .current_dir(&req.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut retries = 0;
    loop {
        stagger.wait().await;
        match attempt(&mut cmd, &bin, &req, &mut sink).await {
            Attempt::SpawnFailed(why) => return sink(RunnerMsg::SpawnFailed(why)),
            Attempt::Exited {
                code,
                stderr_tail,
                produced,
            } => {
                let collided = code != Some(0) && !produced && is_config_contention(&stderr_tail);
                if collided && retries < contention::RETRIES {
                    let spread = random_below(Duration::from_millis(250));
                    tokio::time::sleep(contention::backoff(retries, spread)).await;
                    retries += 1;
                    continue;
                }
                return sink(RunnerMsg::Exited { code, stderr_tail });
            }
        }
    }
}

/// Starts `claude` once and follows it to its end. The process tree dies
/// with this future, so aborting the job's task kills every descendant.
async fn attempt(cmd: &mut Command, bin: &Path, req: &RunRequest, sink: &mut RunSink) -> Attempt {
    let (mut child, _tree) = match spawn_tokio(cmd) {
        Ok(started) => started,
        Err(e) => return Attempt::SpawnFailed(format!("{}: {e}", bin.display())),
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

    let mut produced = false;
    if let Some(stdout) = child.stdout.take() {
        let mut reader = BufReader::new(stdout);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    produced = true;
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
    Attempt::Exited {
        code,
        stderr_tail,
        produced,
    }
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
