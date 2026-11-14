//! `swarmux hook canary <mode> [--log <file>]`: a PreToolUse hook that does
//! what a canary needs, and nothing else.
//!
//! The canaries (see `claude_cli::runtime::canary`) launch Claude Code with
//! this hook in place of the Guard's and watch what Claude Code does with
//! each answer: a denial, a failing exit code, silence that outlasts the
//! hook's timeout, output that is not JSON. The mode is an argument in the
//! generated settings, never an environment variable: nothing the agent or
//! a repository can set changes what the Guard's own hook does.

use std::io::{Read, Write};
use std::path::Path;

use serde_json::json;

/// The reason a denying canary gives; it comes back in the tool result,
/// which is how a canary knows the denial was this hook's.
pub const CANARY_REASON: &str = "swarmux canary: denied on purpose";

/// What a canary hook does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanaryAnswer {
    /// Print this and exit 0.
    Print(String),
    /// Exit with this code, printing nothing.
    Exit(i32),
    /// Say nothing and never finish: the hook's timeout ends it.
    Hang,
}

/// The answer for `mode`: `deny`, `exit1`, `badjson` or `hang`. An unknown
/// mode denies, so a typo never lets a call through.
pub fn canary_answer(mode: &str) -> CanaryAnswer {
    match mode {
        "exit1" => CanaryAnswer::Exit(1),
        "badjson" => CanaryAnswer::Print("{not json".to_string()),
        "hang" => CanaryAnswer::Hang,
        _ => CanaryAnswer::Print(
            json!({ "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": CANARY_REASON,
            }})
            .to_string(),
        ),
    }
}

/// The line left in the log for one call: the tool, and the subagent that
/// made it when the input says so.
fn log_line(mode: &str, stdin: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(stdin).unwrap_or_default();
    let text = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default();
    json!({
        "mode": mode,
        "tool": text("tool_name"),
        "agent_id": text("agent_id"),
        "agent_type": text("agent_type"),
    })
    .to_string()
}

/// Runs the canary hook with this process's stdin and stdout. `args` are
/// what follows `canary`: the mode, then optionally `--log <file>`.
pub fn canary_main(args: &[String]) {
    let mode = args.first().map_or("deny", String::as_str);
    let log = args
        .iter()
        .position(|a| a == "--log")
        .and_then(|i| args.get(i + 1));
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    if let Some(log) = log {
        append_line(Path::new(log), &log_line(mode, &input));
    }
    match canary_answer(mode) {
        CanaryAnswer::Print(out) => {
            let mut stdout = std::io::stdout().lock();
            let _ = writeln!(stdout, "{out}");
            let _ = stdout.flush();
        }
        CanaryAnswer::Exit(code) => std::process::exit(code),
        CanaryAnswer::Hang => std::thread::sleep(std::time::Duration::from_secs(3600)),
    }
}

fn append_line(path: &Path, line: &str) {
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_prints_a_pre_tool_use_denial_with_the_reason() {
        let CanaryAnswer::Print(out) = canary_answer("deny") else {
            panic!("deny prints")
        };
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(
            v["hookSpecificOutput"]["permissionDecisionReason"],
            CANARY_REASON
        );
    }

    #[test]
    fn the_failure_modes_fail_in_their_own_way() {
        assert_eq!(canary_answer("exit1"), CanaryAnswer::Exit(1));
        assert_eq!(canary_answer("hang"), CanaryAnswer::Hang);
        let CanaryAnswer::Print(out) = canary_answer("badjson") else {
            panic!("badjson prints")
        };
        assert!(serde_json::from_str::<serde_json::Value>(&out).is_err());
    }

    #[test]
    fn a_mode_nobody_knows_denies() {
        assert_eq!(canary_answer("deny"), canary_answer("allow"));
        assert_eq!(canary_answer(""), canary_answer("deny"));
    }

    #[test]
    fn the_log_line_names_the_tool_and_the_subagent() {
        let line = log_line(
            "deny",
            r#"{"tool_name":"Bash","agent_id":"a1","agent_type":"general-purpose"}"#,
        );
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["tool"], "Bash");
        assert_eq!(v["agent_id"], "a1");
        let empty: serde_json::Value = serde_json::from_str(&log_line("deny", "x")).unwrap();
        assert_eq!(empty["tool"], "");
        assert_eq!(empty["agent_id"], "");
    }
}
