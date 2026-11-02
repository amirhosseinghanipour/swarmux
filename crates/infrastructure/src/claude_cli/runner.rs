//! `AgentRunner` adapter: runs each job as a `claude -p` child process.

use std::path::PathBuf;
use std::process::Stdio;

use swarmux_application::ports::{AgentRunner, RunRequest, RunSink, RunnerMsg};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::runtime::Handle;

use super::command::{build_args, user_message};
use super::parse::parse_line;

/// How much of the end of stderr is kept for error messages.
const STDERR_TAIL: usize = 2048;

pub struct ClaudeCli {
    bin: PathBuf,
    env: Vec<(String, String)>,
    rt: Handle,
}

impl ClaudeCli {
    pub fn new(bin: PathBuf, rt: Handle) -> ClaudeCli {
        ClaudeCli {
            bin,
            env: Vec::new(),
            rt,
        }
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
        let env = self.env.clone();
        self.rt.spawn(run(bin, env, request, sink));
    }
}

async fn run(bin: PathBuf, env: Vec<(String, String)>, req: RunRequest, sink: RunSink) {
    let mut cmd = Command::new(&bin);
    cmd.args(build_args(&req))
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
