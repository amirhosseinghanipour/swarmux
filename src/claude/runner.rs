//! Runs one job: spawns `claude`, sends the prompt, streams parsed events back.

use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;

use super::command::{JobSpec, build_args, user_message};
use super::event::{Event, parse_line_all};

/// How much of the end of stderr is kept for error messages.
const STDERR_TAIL: usize = 2048;

#[derive(Debug, Clone, PartialEq)]
pub enum RunnerMsg {
    Event(Event),
    Exited {
        code: Option<i32>,
        stderr_tail: String,
    },
    SpawnFailed(String),
}

/// Runs the job to completion. Messages stop early if the receiver is dropped;
/// the child is then killed (`kill_on_drop`).
pub async fn run_job(spec: JobSpec, prompt: String, tx: UnboundedSender<RunnerMsg>) {
    let mut cmd = Command::new(&spec.claude_bin);
    cmd.args(build_args(&spec))
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .current_dir(&spec.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flash per job

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            let _ = tx.send(RunnerMsg::SpawnFailed(format!(
                "{}: {e}",
                spec.claude_bin.display()
            )));
            return;
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let msg = user_message(&prompt) + "\n";
        // If the child already exited, the exit status explains why.
        let _ = stdin.write_all(msg.as_bytes()).await;
    } // stdin dropped here: claude sees EOF after the one message

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
            last_chars(&String::from_utf8_lossy(&tail), STDERR_TAIL)
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
                    for ev in parse_line_all(&String::from_utf8_lossy(&line)) {
                        if tx.send(RunnerMsg::Event(ev)).is_err() {
                            return; // app is gone; dropping `child` kills it
                        }
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
    let _ = tx.send(RunnerMsg::Exited { code, stderr_tail });
}

fn last_chars(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut start = s.len() - max_bytes;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}
