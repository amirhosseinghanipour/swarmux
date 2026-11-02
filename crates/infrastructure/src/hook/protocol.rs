//! What travels between `swarmux hook pre-tool` and the running app, and
//! what the hook prints for Claude Code.
//!
//! Both directions are one line of JSON.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use swarmux_application::ports::HookRequest;
use swarmux_domain::{Decision, JobId, Verdict};

/// Longest tool input kept for display, in characters.
const INPUT_MAX: usize = 2000;

/// Hook client → app.
#[derive(Debug, Serialize, Deserialize)]
pub struct WireRequest {
    pub job_id: Option<i64>,
    pub tool_name: String,
    #[serde(default)]
    pub tool_input: Value,
    #[serde(default)]
    pub cwd: String,
}

/// App → hook client.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireReply {
    /// "allow", "deny" or "pass".
    pub decision: String,
    #[serde(default)]
    pub reason: String,
}

impl WireRequest {
    /// Builds the request from the JSON Claude Code gives the hook on stdin.
    pub fn from_hook_input(stdin: &str, job_id: Option<i64>) -> Option<WireRequest> {
        let v: Value = serde_json::from_str(stdin).ok()?;
        Some(WireRequest {
            job_id,
            tool_name: v.get("tool_name")?.as_str()?.to_string(),
            tool_input: v.get("tool_input").cloned().unwrap_or(Value::Null),
            cwd: v
                .get("cwd")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
    }

    pub fn into_hook_request(self, request_id: u64) -> HookRequest {
        let subject = subject_of(&self.tool_name, &self.tool_input);
        let mut input = self.tool_input.to_string();
        if let Some((cut, _)) = input.char_indices().nth(INPUT_MAX) {
            input.truncate(cut);
            input.push('…');
        }
        HookRequest {
            request_id,
            job_id: self.job_id.map(JobId),
            tool_name: self.tool_name,
            subject,
            input,
            cwd: self.cwd,
        }
    }
}

impl WireReply {
    pub fn from_decision(d: &Decision) -> WireReply {
        let decision = match d.verdict {
            Verdict::Allow => "allow",
            Verdict::Deny => "deny",
            // The app never forwards "ask": in `claude -p` it would mean deny.
            Verdict::Ask | Verdict::Pass => "pass",
        };
        let reason = match (&d.rule, d.verdict) {
            (Some(rule), Verdict::Allow) => format!("swarmux guard allowed it: {rule}"),
            (Some(rule), Verdict::Deny) => format!("swarmux guard denied it: {rule}"),
            (None, Verdict::Deny) => "swarmux guard denied it".to_string(),
            _ => String::new(),
        };
        WireReply {
            decision: decision.into(),
            reason,
        }
    }

    pub fn pass() -> WireReply {
        WireReply {
            decision: "pass".into(),
            reason: String::new(),
        }
    }

    /// What the hook prints: Claude Code's PreToolUse JSON, or nothing to
    /// leave the call to Claude Code's own rules.
    pub fn hook_output(&self) -> Option<String> {
        let decision = match self.decision.as_str() {
            "allow" => "allow",
            "deny" => "deny",
            _ => return None,
        };
        Some(
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": decision,
                    "permissionDecisionReason": self.reason,
                }
            })
            .to_string(),
        )
    }
}

/// What a tool call acts on: the command, file path, URL or search path.
pub fn subject_of(tool: &str, input: &Value) -> String {
    let field = |k: &str| input.get(k).and_then(Value::as_str).map(str::to_string);
    let subject = match tool {
        "Bash" | "PowerShell" => field("command"),
        "Read" | "Edit" | "Write" | "MultiEdit" => field("file_path"),
        "NotebookEdit" | "NotebookRead" => field("notebook_path").or_else(|| field("file_path")),
        "Glob" | "Grep" | "LS" => field("path"),
        "WebFetch" => field("url"),
        "WebSearch" => field("query"),
        "Task" | "Agent" => field("subagent_type"),
        _ => None,
    };
    subject.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_from_claude_hook_input() {
        let stdin = r#"{"session_id":"s","hook_event_name":"PreToolUse","tool_name":"Bash",
            "tool_input":{"command":"cargo test","description":"run"},"cwd":"C:\\w"}"#;
        let req = WireRequest::from_hook_input(stdin, Some(7)).unwrap();
        let hr = req.into_hook_request(3);
        assert_eq!(hr.request_id, 3);
        assert_eq!(hr.job_id, Some(JobId(7)));
        assert_eq!(hr.subject, "cargo test");
        assert_eq!(hr.cwd, r"C:\w");
        assert!(hr.input.contains("description"));
        assert!(WireRequest::from_hook_input("nope", None).is_none());
        assert!(WireRequest::from_hook_input("{}", None).is_none());
    }

    #[test]
    fn long_inputs_are_shortened() {
        let req = WireRequest {
            job_id: None,
            tool_name: "Write".into(),
            tool_input: json!({"file_path": "a", "content": "x".repeat(5000)}),
            cwd: String::new(),
        };
        let hr = req.into_hook_request(1);
        assert_eq!(hr.input.chars().count(), INPUT_MAX + 1);
        assert_eq!(hr.subject, "a");
    }

    #[test]
    fn subjects_per_tool() {
        let s = |tool: &str, v: Value| subject_of(tool, &v);
        assert_eq!(s("Edit", json!({"file_path": "/a"})), "/a");
        assert_eq!(s("WebFetch", json!({"url": "https://x"})), "https://x");
        assert_eq!(s("Grep", json!({"pattern": "p"})), "");
        assert_eq!(s("mcp__x", json!({"a": 1})), "");
    }

    #[test]
    fn hook_output_matches_claude_code() {
        let allow = WireReply::from_decision(&Decision::new(Verdict::Allow, Some("Bash".into())));
        let v: Value = serde_json::from_str(&allow.hook_output().unwrap()).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "allow");
        assert_eq!(
            v["hookSpecificOutput"]["permissionDecisionReason"],
            "swarmux guard allowed it: Bash"
        );
        let deny = WireReply::from_decision(&Decision::new(Verdict::Deny, None));
        assert!(deny.hook_output().unwrap().contains("\"deny\""));
        assert_eq!(WireReply::pass().hook_output(), None);
        let ask = WireReply::from_decision(&Decision::new(Verdict::Ask, None));
        assert_eq!(ask.hook_output(), None);
    }
}
