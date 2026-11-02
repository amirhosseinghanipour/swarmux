//! Builds the `claude -p` invocation for a run request.

use swarmux_application::ports::RunRequest;

pub fn build_args(req: &RunRequest) -> Vec<String> {
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
    args
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
    use swarmux_domain::SessionId;
    use uuid::Uuid;

    fn req(resume: bool, fork: bool) -> RunRequest {
        RunRequest {
            cwd: "/w".into(),
            session_id: SessionId(Uuid::nil()),
            resume,
            fork,
            model: "sonnet".into(),
            permission_mode: "default".into(),
            prompt: "hi".into(),
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
