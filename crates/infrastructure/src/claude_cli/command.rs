//! Builds the `claude -p` invocation for a run request.

use swarmux_application::ports::RunRequest;
use swarmux_domain::REVIEW_SYSTEM;

/// Tools a reviewer may use: reading only.
pub const REVIEW_TOOLS: &str = "Read,Grep,Glob";
/// Turns a reviewer gets to read files before answering.
const REVIEW_TURNS: &str = "12";
/// Longest argument shown in full by [`display`].
const SHOWN_ARG_MAX: usize = 40;

pub fn build_args(req: &RunRequest) -> Vec<String> {
    if req.review {
        return review_args(req);
    }
    let mut args: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--input-format",
        "stream-json",
        "--verbose",
        "--forward-subagent-text",
    ]
    .map(String::from)
    .to_vec();

    let id = req.session_id.to_string();
    if req.resume {
        args.extend(["--resume".into(), id]);
        if req.fork {
            args.push("--fork-session".into());
        }
    } else {
        args.extend(["--session-id".into(), id]);
    }
    args.extend(["--model".into(), req.model.clone()]);
    args.extend(["--permission-mode".into(), req.permission_mode.clone()]);
    if req.lean {
        // Skips the user's own settings (their hooks and plugins); swarmux's
        // own `--settings` still apply.
        args.extend(["--setting-sources".into(), "project,local".into()]);
    }
    push_budget(&mut args, req);
    args
}

/// `--max-budget-usd` when the run has a dollar cap.
fn push_budget(args: &mut Vec<String>, req: &RunRequest) {
    if let Some(usd) = req.max_budget_usd.filter(|u| u.is_finite() && *u > 0.0) {
        args.extend(["--max-budget-usd".into(), format!("{usd:.2}")]);
    }
}

/// A review run: read-only tools, no session file, none of the user's
/// settings or MCP servers, and the reviewer's system prompt.
fn review_args(req: &RunRequest) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--input-format",
        "stream-json",
        "--verbose",
        "--model",
        &req.model,
        "--tools",
        REVIEW_TOOLS,
        "--permission-mode",
        "default",
        "--max-turns",
        REVIEW_TURNS,
        "--setting-sources",
        "project,local",
        "--strict-mcp-config",
        "--no-session-persistence",
        "--system-prompt",
        REVIEW_SYSTEM,
    ]
    .map(String::from)
    .to_vec();
    push_budget(&mut args, req);
    args
}

/// `claude` and its arguments as one line for display: long or JSON
/// arguments (the guard hook's settings, system prompts) are elided.
pub fn display(args: &[String]) -> String {
    let mut line = String::from("claude");
    for a in args {
        line.push(' ');
        if a.is_empty() {
            line.push_str("\"\"");
        } else if a.starts_with('{') || a.chars().count() > SHOWN_ARG_MAX {
            line.push('…');
        } else if a.contains(char::is_whitespace) {
            line.push_str(&format!("\"{a}\""));
        } else {
            line.push_str(a);
        }
    }
    line
}

/// The stream-json user message carrying the prompt (one line, no newline).
pub fn user_message(prompt: &str) -> String {
    serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": prompt }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_domain::{JobId, SessionId};
    use uuid::Uuid;

    fn req(resume: bool, fork: bool) -> RunRequest {
        RunRequest {
            job_id: JobId(1),
            cwd: "/w".into(),
            session_id: SessionId(Uuid::nil()),
            resume,
            fork,
            model: "sonnet".into(),
            permission_mode: "default".into(),
            prompt: "hi".into(),
            lean: false,
            review: false,
            env: Vec::new(),
            max_budget_usd: None,
            board: false,
        }
    }

    fn has_pair(args: &[String], a: &str, b: &str) -> bool {
        args.windows(2).any(|w| w[0] == a && w[1] == b)
    }

    #[test]
    fn first_job_uses_session_id() {
        let args = build_args(&req(false, false));
        assert!(has_pair(&args, "--session-id", &Uuid::nil().to_string()));
        assert!(!args.contains(&"--resume".to_string()));
    }

    #[test]
    fn later_job_resumes() {
        let args = build_args(&req(true, false));
        assert!(has_pair(&args, "--resume", &Uuid::nil().to_string()));
        assert!(!args.contains(&"--session-id".to_string()));
    }

    #[test]
    fn fork_adds_fork_session_only_when_resuming() {
        assert!(build_args(&req(true, true)).contains(&"--fork-session".to_string()));
        assert!(!build_args(&req(false, true)).contains(&"--fork-session".to_string()));
    }

    #[test]
    fn always_has_stream_flags_and_model() {
        let args = build_args(&req(false, false));
        assert_eq!(
            args[..6],
            [
                "-p",
                "--output-format",
                "stream-json",
                "--input-format",
                "stream-json",
                "--verbose"
            ]
        );
        assert!(has_pair(&args, "--model", "sonnet"));
        assert!(has_pair(&args, "--permission-mode", "default"));
        assert!(args.contains(&"--forward-subagent-text".to_string()));
    }

    #[test]
    fn dollar_cap_becomes_max_budget_usd() {
        assert!(!build_args(&req(false, false)).contains(&"--max-budget-usd".to_string()));
        let capped = RunRequest {
            max_budget_usd: Some(2.4),
            board: false,
            ..req(false, false)
        };
        assert!(has_pair(&build_args(&capped), "--max-budget-usd", "2.40"));
        let review = RunRequest {
            review: true,
            ..capped.clone()
        };
        assert!(has_pair(&build_args(&review), "--max-budget-usd", "2.40"));
        for bad in [0.0, -1.0, f64::NAN] {
            let none = RunRequest {
                max_budget_usd: Some(bad),
                board: false,
                ..req(false, false)
            };
            assert!(!build_args(&none).contains(&"--max-budget-usd".to_string()));
        }
    }

    #[test]
    fn lean_skips_user_settings() {
        assert!(!build_args(&req(false, false)).contains(&"--setting-sources".to_string()));
        let lean = RunRequest {
            lean: true,
            ..req(false, false)
        };
        assert!(has_pair(
            &build_args(&lean),
            "--setting-sources",
            "project,local"
        ));
    }

    #[test]
    fn review_runs_are_read_only_and_sessionless() {
        let review = RunRequest {
            review: true,
            env: Vec::new(),
            resume: true,
            model: "haiku".into(),
            ..req(true, true)
        };
        let args = build_args(&review);
        assert!(has_pair(&args, "--tools", "Read,Grep,Glob"));
        assert!(has_pair(&args, "--model", "haiku"));
        assert!(has_pair(&args, "--setting-sources", "project,local"));
        assert!(has_pair(&args, "--system-prompt", REVIEW_SYSTEM));
        assert!(args.contains(&"--no-session-persistence".to_string()));
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        for absent in [
            "--resume",
            "--session-id",
            "--fork-session",
            "--forward-subagent-text",
        ] {
            assert!(!args.contains(&absent.to_string()), "{absent}");
        }
    }

    #[test]
    fn display_elides_long_and_json_arguments() {
        let args: Vec<String> = ["-p", "--settings", "{\"hooks\":{}}", "--tools", "", "a b"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            display(&args),
            "claude -p --settings … --tools \"\" \"a b\""
        );
        let long = vec!["x".repeat(SHOWN_ARG_MAX + 1)];
        assert_eq!(display(&long), "claude …");
    }

    #[test]
    fn prompt_message_round_trips_special_chars() {
        let p = "say \"hi\"\nnext — ñ 🐝";
        let msg = user_message(p);
        assert!(!msg.contains('\n'));
        let v: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(v["type"], "user");
        assert_eq!(v["message"]["role"], "user");
        assert_eq!(v["message"]["content"], p);
    }
}
