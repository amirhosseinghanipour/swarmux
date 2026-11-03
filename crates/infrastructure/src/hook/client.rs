//! `swarmux hook <event>`: the process Claude Code runs for each hook event.
//!
//! It forwards the event to the running app and prints the answer. Whatever
//! goes wrong, it prints nothing and exits 0, so Claude Code carries on as
//! if the hook weren't there.

use std::io::{BufRead, BufReader, Read, Write};

use swarmux_domain::HookEvent;

use super::protocol::{WireReply, WireRequest};
use super::transport::{ENV_JOB, ENV_PIPE, connect};

/// Runs `swarmux hook <name>` with this process's stdin, stdout and
/// environment. An unknown `name` prints nothing.
pub fn hook_main(name: &str) {
    let Some(event) = HookEvent::parse(name) else {
        return;
    };
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return;
    }
    let pipe = std::env::var(ENV_PIPE).ok();
    let job = std::env::var(ENV_JOB).ok();
    if let Some(out) = run_hook(event, &input, pipe.as_deref(), job.as_deref()) {
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{out}");
        let _ = stdout.flush();
    }
}

/// Runs the PreToolUse hook with this process's stdin, stdout and environment.
pub fn pre_tool_main() {
    hook_main(HookEvent::PreToolUse.as_str());
}

/// What the hook should print for `event` with hook input `stdin`, telling
/// the app at socket `pipe` about job `job`. `None` means print nothing.
///
/// Events Claude Code doesn't wait for are sent and forgotten.
pub fn run_hook(
    event: HookEvent,
    stdin: &str,
    pipe: Option<&str>,
    job: Option<&str>,
) -> Option<String> {
    if event == HookEvent::PreToolUse {
        return pre_tool(stdin, pipe, job);
    }
    let pipe = pipe.map(str::trim).filter(|p| !p.is_empty())?;
    let job_id = job.and_then(|j| j.trim().parse::<i64>().ok());
    let request = WireRequest::from_event_input(event, stdin, job_id)?;
    exchange(&request, pipe, event.is_blocking())?.hook_output()
}

/// What the hook should print for the PreToolUse input `stdin`, asking the
/// app at socket `pipe` about job `job`. `None` means print nothing.
pub fn pre_tool(stdin: &str, pipe: Option<&str>, job: Option<&str>) -> Option<String> {
    let pipe = pipe.map(str::trim).filter(|p| !p.is_empty())?;
    let job_id = job.and_then(|j| j.trim().parse::<i64>().ok());
    let request = WireRequest::from_hook_input(stdin, job_id)?;
    exchange(&request, pipe, true)?.hook_output()
}

/// Sends `request` without waiting for an answer. `false` when the app
/// couldn't be reached.
pub(super) fn send(request: &WireRequest, pipe: &str) -> bool {
    let Ok(mut line) = serde_json::to_string(request) else {
        return false;
    };
    line.push('\n');
    let Ok(mut conn) = connect(pipe) else {
        return false;
    };
    conn.write_all(line.as_bytes()).is_ok() && conn.flush().is_ok()
}

/// Sends `request`; with `wait`, also reads the app's reply.
fn exchange(request: &WireRequest, pipe: &str, wait: bool) -> Option<WireReply> {
    let mut line = serde_json::to_string(request).ok()?;
    line.push('\n');
    let mut conn = connect(pipe).ok()?;
    conn.write_all(line.as_bytes()).ok()?;
    conn.flush().ok()?;
    if !wait {
        return None;
    }
    let mut answer = String::new();
    BufReader::new(conn).read_line(&mut answer).ok()?;
    serde_json::from_str(&answer).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INPUT: &str = r#"{"tool_name":"Bash","tool_input":{"command":"ls"},"cwd":"/w"}"#;

    #[test]
    fn no_pipe_means_no_output() {
        assert_eq!(pre_tool(INPUT, None, Some("1")), None);
        assert_eq!(pre_tool(INPUT, Some("  "), Some("1")), None);
        assert_eq!(run_hook(HookEvent::Stop, "{}", None, None), None);
    }

    #[test]
    fn unreachable_pipe_or_bad_input_means_no_output() {
        let pipe = super::super::transport::endpoint_for(u32::MAX - 1);
        assert_eq!(pre_tool(INPUT, Some(&pipe), None), None);
        assert_eq!(pre_tool("garbage", Some(&pipe), None), None);
        assert_eq!(run_hook(HookEvent::Stop, "{}", Some(&pipe), None), None);
        assert_eq!(
            run_hook(HookEvent::PostToolUse, "garbage", Some(&pipe), None),
            None
        );
    }
}
