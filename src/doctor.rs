use std::process::Command;

/// Prints whether Claude Code is installed and which version.
pub fn run() -> anyhow::Result<()> {
    match claude_version() {
        Some(v) => println!("ok   claude found: {v}"),
        None => println!(
            "fail claude not found on PATH — install Claude Code: https://docs.claude.com/claude-code"
        ),
    }
    Ok(())
}

fn claude_version() -> Option<String> {
    // On Windows `claude` is usually a .cmd shim, so go through the shell.
    let output = if cfg!(windows) {
        Command::new("cmd")
            .args(["/C", "claude", "--version"])
            .output()
    } else {
        Command::new("claude").arg("--version").output()
    }
    .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}
