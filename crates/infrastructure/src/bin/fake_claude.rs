//! Test stand-in for `claude` (built only with `--features fake-claude`).
//!
//! Env vars: FAKE_CLAUDE_FIXTURE (file printed to stdout), FAKE_CLAUDE_ECHO_FILE
//! (receives {"stdin","cwd","args"} as JSON), FAKE_CLAUDE_STDERR, FAKE_CLAUDE_EXIT.

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
    let code = std::env::var("FAKE_CLAUDE_EXIT")
        .ok()
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    std::process::exit(code);
}
