//! `swarmux hook pre-tool`: the process Claude Code runs before each tool call.
//!
//! It forwards the call to the running app and prints the answer. Whatever
//! goes wrong, it prints nothing and exits 0, so Claude Code's own rules apply.

use std::io::{BufRead, BufReader, Read, Write};

use super::protocol::{WireReply, WireRequest};
use super::transport::{ENV_JOB, ENV_PIPE, connect};

/// Runs the hook with this process's stdin, stdout and environment.
pub fn pre_tool_main() {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return;
    }
    let pipe = std::env::var(ENV_PIPE).ok();
    let job = std::env::var(ENV_JOB).ok();
    if let Some(out) = pre_tool(&input, pipe.as_deref(), job.as_deref()) {
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{out}");
        let _ = stdout.flush();
    }
}

/// What the hook should print for the hook input `stdin`, asking the app at
/// socket `pipe` about job `job`. `None` means print nothing.
pub fn pre_tool(stdin: &str, pipe: Option<&str>, job: Option<&str>) -> Option<String> {
    let pipe = pipe.map(str::trim).filter(|p| !p.is_empty())?;
    let job_id = job.and_then(|j| j.trim().parse::<i64>().ok());
    let request = WireRequest::from_hook_input(stdin, job_id)?;
    let mut line = serde_json::to_string(&request).ok()?;
    line.push('\n');

    let mut conn = connect(pipe).ok()?;
    conn.write_all(line.as_bytes()).ok()?;
    conn.flush().ok()?;
    let mut answer = String::new();
    BufReader::new(conn).read_line(&mut answer).ok()?;
    let reply: WireReply = serde_json::from_str(&answer).ok()?;
    reply.hook_output()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INPUT: &str = r#"{"tool_name":"Bash","tool_input":{"command":"ls"},"cwd":"/w"}"#;

    #[test]
    fn no_pipe_means_no_output() {
        assert_eq!(pre_tool(INPUT, None, Some("1")), None);
        assert_eq!(pre_tool(INPUT, Some("  "), Some("1")), None);
    }

    #[test]
    fn unreachable_pipe_or_bad_input_means_no_output() {
        let pipe = super::super::transport::endpoint_for(u32::MAX - 1);
        assert_eq!(pre_tool(INPUT, Some(&pipe), None), None);
        assert_eq!(pre_tool("garbage", Some(&pipe), None), None);
    }
}
