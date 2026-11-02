//! Builds the `claude -p` invocation for a job (spec section 4.1).

use std::path::PathBuf;

use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct JobSpec {
    pub claude_bin: PathBuf,
    pub cwd: PathBuf,
    pub session_id: Uuid,
    /// Continue an existing session (`--resume`) instead of starting one (`--session-id`).
    pub resume: bool,
    /// With `resume`, branch into a new session id (`--fork-session`).
    pub fork: bool,
    pub model: String,
    pub permission_mode: String,
    /// Extra environment for the child process.
    pub env: Vec<(String, String)>,
}

pub fn build_args(spec: &JobSpec) -> Vec<String> {
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

    let id = spec.session_id.to_string();
    if spec.resume {
        args.extend(["--resume".into(), id]);
        if spec.fork {
            args.push("--fork-session".into());
        }
    } else {
        args.extend(["--session-id".into(), id]);
    }
    args.extend(["--model".into(), spec.model.clone()]);
    args.extend(["--permission-mode".into(), spec.permission_mode.clone()]);
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

    fn spec(resume: bool, fork: bool) -> JobSpec {
        JobSpec {
            claude_bin: "claude".into(),
            cwd: "/w".into(),
            session_id: Uuid::nil(),
            resume,
            fork,
            model: "sonnet".into(),
            permission_mode: "default".into(),
            env: Vec::new(),
        }
    }

    fn has_pair(args: &[String], a: &str, b: &str) -> bool {
        args.windows(2).any(|w| w[0] == a && w[1] == b)
    }

    #[test]
    fn first_job_uses_session_id() {
        let args = build_args(&spec(false, false));
        assert!(has_pair(&args, "--session-id", &Uuid::nil().to_string()));
        assert!(!args.contains(&"--resume".to_string()));
    }

    #[test]
    fn later_job_resumes() {
        let args = build_args(&spec(true, false));
        assert!(has_pair(&args, "--resume", &Uuid::nil().to_string()));
        assert!(!args.contains(&"--session-id".to_string()));
    }

    #[test]
    fn fork_adds_fork_session_only_when_resuming() {
        assert!(build_args(&spec(true, true)).contains(&"--fork-session".to_string()));
        assert!(!build_args(&spec(false, true)).contains(&"--fork-session".to_string()));
    }

    #[test]
    fn always_has_stream_flags_and_model() {
        let args = build_args(&spec(false, false));
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
