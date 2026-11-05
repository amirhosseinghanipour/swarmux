//! Test stand-in for `claude` (built only with `--features fake-claude`).
//!
//! Env vars: FAKE_CLAUDE_FIXTURE (file printed to stdout), FAKE_CLAUDE_ECHO_FILE
//! (receives {"stdin","cwd","args","hook_pipe","job_id"} as JSON), FAKE_CLAUDE_STDERR, FAKE_CLAUDE_EXIT,
//! FAKE_CLAUDE_LINE_DELAY_MS (wait before each line of the fixture),
//! FAKE_CLAUDE_SLEEP_MS (wait before exiting) and FAKE_CLAUDE_DONE_FILE (written
//! after the wait, so tests can tell whether the process was killed).
//! FAKE_CLAUDE_ARGS_FILE gets one line of arguments per start.
//! FAKE_CLAUDE_START_FILE gets one line (nanoseconds since the epoch) per
//! start, FAKE_CLAUDE_PIDS_FILE one line per process id, and
//! FAKE_CLAUDE_GRANDCHILDREN=n starts n long-lived copies of this program.
//! FAKE_CLAUDE_CONTENTION=n with FAKE_CLAUDE_COUNTER_FILE makes the first n
//! starts fail as if another session held `.claude.json`.

use std::io::{Read, Write};

/// Appends one line to the file named by the environment variable `var`.
fn append_line(var: &str, line: &str) {
    if let Ok(path) = std::env::var(var) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        // One write, so lines from different processes never interleave.
        file.write_all(format!("{line}\n").as_bytes()).unwrap();
    }
}

fn main() {
    if std::env::var_os("FAKE_CLAUDE_START_FILE").is_some() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        append_line("FAKE_CLAUDE_START_FILE", &now.to_string());
    }
    append_line("FAKE_CLAUDE_PIDS_FILE", &std::process::id().to_string());
    if std::env::var_os("FAKE_CLAUDE_ARGS_FILE").is_some() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        append_line("FAKE_CLAUDE_ARGS_FILE", &args.join(" "));
    }
    if std::env::var_os("FAKE_CLAUDE_GRANDCHILD").is_some() {
        std::thread::sleep(std::time::Duration::from_secs(120));
        return;
    }
    if let Some(n) = std::env::var("FAKE_CLAUDE_GRANDCHILDREN")
        .ok()
        .and_then(|n| n.parse::<usize>().ok())
    {
        let exe = std::env::current_exe().unwrap();
        // They are meant to outlive this process unless something kills them.
        #[allow(clippy::zombie_processes)]
        for _ in 0..n {
            std::process::Command::new(&exe)
                .env("FAKE_CLAUDE_GRANDCHILD", "1")
                .env_remove("FAKE_CLAUDE_START_FILE")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
        }
    }
    if let (Some(n), Ok(path)) = (
        std::env::var("FAKE_CLAUDE_CONTENTION")
            .ok()
            .and_then(|n| n.parse::<usize>().ok()),
        std::env::var("FAKE_CLAUDE_COUNTER_FILE"),
    ) {
        let seen = std::fs::read_to_string(&path).map_or(0, |t| t.lines().count());
        append_line("FAKE_CLAUDE_COUNTER_FILE", "start");
        if seen < n {
            eprint!(
                "Claude configuration file at C:\\Users\\a\\.claude.json is corrupted: Unexpected end of JSON input"
            );
            std::process::exit(1);
        }
    }
    let mut stdin = String::new();
    let _ = std::io::stdin().read_to_string(&mut stdin);

    if let Ok(path) = std::env::var("FAKE_CLAUDE_ECHO_FILE") {
        let echo = serde_json::json!({
            "stdin": stdin,
            "cwd": std::env::current_dir().unwrap().to_string_lossy(),
            "args": std::env::args().skip(1).collect::<Vec<_>>(),
            "hook_pipe": std::env::var("SWARMUX_HOOK_PIPE").ok(),
            "job_id": std::env::var("SWARMUX_JOB_ID").ok(),
        });
        std::fs::write(path, echo.to_string()).unwrap();
    }
    if let Ok(path) = std::env::var("FAKE_CLAUDE_FIXTURE") {
        let text = std::fs::read_to_string(path).unwrap();
        match std::env::var("FAKE_CLAUDE_LINE_DELAY_MS")
            .ok()
            .and_then(|ms| ms.parse::<u64>().ok())
        {
            // One step at a time, as a real run streams them.
            Some(ms) => {
                let mut out = std::io::stdout().lock();
                for line in text.lines() {
                    std::thread::sleep(std::time::Duration::from_millis(ms));
                    writeln!(out, "{line}").unwrap();
                    out.flush().unwrap();
                }
            }
            None => print!("{text}"),
        }
    }
    if let Ok(msg) = std::env::var("FAKE_CLAUDE_STDERR") {
        eprint!("{msg}");
    }
    if let Some(ms) = std::env::var("FAKE_CLAUDE_SLEEP_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
    {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
    if let Ok(path) = std::env::var("FAKE_CLAUDE_DONE_FILE") {
        std::fs::write(path, "done").unwrap();
    }
    let code = std::env::var("FAKE_CLAUDE_EXIT")
        .ok()
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    std::process::exit(code);
}
