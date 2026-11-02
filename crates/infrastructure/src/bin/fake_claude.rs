//! Test stand-in for `claude` (built only with `--features fake-claude`).
//!
//! Env vars: FAKE_CLAUDE_FIXTURE (file printed to stdout), FAKE_CLAUDE_ECHO_FILE
//! (receives {"stdin","cwd","args"} as JSON), FAKE_CLAUDE_STDERR, FAKE_CLAUDE_EXIT,
//! FAKE_CLAUDE_SLEEP_MS (wait before exiting) and FAKE_CLAUDE_DONE_FILE (written
//! after the wait, so tests can tell whether the process was killed).

use std::io::Read;

fn main() {
    let mut stdin = String::new();
    let _ = std::io::stdin().read_to_string(&mut stdin);

    if let Ok(path) = std::env::var("FAKE_CLAUDE_ECHO_FILE") {
        let echo = serde_json::json!({
            "stdin": stdin,
            "cwd": std::env::current_dir().unwrap().to_string_lossy(),
            "args": std::env::args().skip(1).collect::<Vec<_>>(),
        });
        std::fs::write(path, echo.to_string()).unwrap();
    }
    if let Ok(path) = std::env::var("FAKE_CLAUDE_FIXTURE") {
        print!("{}", std::fs::read_to_string(path).unwrap());
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
