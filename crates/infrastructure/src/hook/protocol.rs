//! What travels between `swarmux hook pre-tool` and the running app, and
//! what the hook prints for Claude Code.
//!
//! Both directions are one line of JSON.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use swarmux_application::HookNotice;
use swarmux_application::ports::HookRequest;
use swarmux_domain::ledger::{hex, sha256};
use swarmux_domain::{Decision, HookEvent, JobId, Verdict};

/// Longest tool input kept for display, in characters.
const INPUT_MAX: usize = 2000;
/// Longest hook input carried over the socket, in bytes.
pub const RAW_MAX: usize = 16 * 1024;
/// Longest one-line summary, in characters.
const SUMMARY_MAX: usize = 200;
/// Longest detail text (a last message), in characters.
const DETAIL_MAX: usize = 4000;

/// Hook client → app.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct WireRequest {
    pub job_id: Option<i64>,
    pub tool_name: String,
    #[serde(default)]
    pub tool_input: Value,
    #[serde(default)]
    pub cwd: String,
    /// The hook event; absent in requests from the first hook client, which
    /// only knew PreToolUse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// The hook input as received, cut at [`RAW_MAX`] bytes.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw: String,
    /// SHA-256 of the whole hook input (hex).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub raw_sha256: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub raw_len: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// App → hook client.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireReply {
    /// "allow", "deny" or "pass".
    pub decision: String,
    #[serde(default)]
    pub reason: String,
    /// What the hook prints for events other than PreToolUse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

impl WireRequest {
    /// Builds the request from the JSON Claude Code gives the hook on stdin.
    pub fn from_hook_input(stdin: &str, job_id: Option<i64>) -> Option<WireRequest> {
        let v: Value = serde_json::from_str(stdin).ok()?;
        Some(WireRequest {
            job_id,
            tool_name: v.get("tool_name")?.as_str()?.to_string(),
            tool_input: v.get("tool_input").cloned().unwrap_or(Value::Null),
            cwd: str_at(&v, "cwd"),
            ..WireRequest::default()
        })
    }

    /// Builds the request for any event other than PreToolUse from the JSON
    /// Claude Code gives the hook on stdin. `None` when the input isn't JSON.
    pub fn from_event_input(
        event: HookEvent,
        stdin: &str,
        job_id: Option<i64>,
    ) -> Option<WireRequest> {
        let v: Value = serde_json::from_str(stdin).ok()?;
        let tool = str_at(&v, "tool_name");
        let subject = subject_of(&tool, v.get("tool_input").unwrap_or(&Value::Null));
        let with_subject = |tool: &str| match (tool.is_empty(), subject.is_empty()) {
            (true, _) => String::new(),
            (false, true) => tool.to_string(),
            (false, false) => format!("{tool} {subject}"),
        };
        let (summary, detail) = match event {
            HookEvent::PostToolUse => {
                let ms = v.get("duration_ms").and_then(Value::as_u64);
                let took = ms.map_or(String::new(), |ms| format!(" ({ms} ms)"));
                (format!("{}{took}", with_subject(&tool)), String::new())
            }
            HookEvent::PostToolUseFailure => {
                let error = str_at(&v, "error");
                (
                    format!("{}: {}", with_subject(&tool), first_line(&error)),
                    error,
                )
            }
            HookEvent::PermissionDenied => {
                let reason = str_at(&v, "reason");
                (with_subject(&tool), reason)
            }
            HookEvent::Stop | HookEvent::SubagentStop => {
                let last = str_at(&v, "last_assistant_message");
                (first_line(&last).to_string(), last)
            }
            HookEvent::SessionStart => {
                let source = str_at(&v, "source");
                (format!("session {source}"), source)
            }
            HookEvent::PreCompact => {
                let trigger = str_at(&v, "trigger");
                (format!("compaction ({trigger})"), trigger)
            }
            HookEvent::Notification => (str_at(&v, "message"), str_at(&v, "notification_type")),
            HookEvent::StopFailure => {
                let error = str_at(&v, "error");
                (error.clone(), error)
            }
            HookEvent::PreToolUse => (with_subject(&tool), String::new()),
        };
        let mut raw = stdin.to_string();
        if raw.len() > RAW_MAX {
            let mut end = RAW_MAX;
            while !raw.is_char_boundary(end) {
                end -= 1;
            }
            raw.truncate(end);
        }
        Some(WireRequest {
            job_id,
            tool_name: tool,
            tool_input: Value::Null,
            cwd: str_at(&v, "cwd"),
            event: Some(event.as_str().to_string()),
            session_id: str_at(&v, "session_id"),
            summary: cut(&summary, SUMMARY_MAX),
            detail: cut(&detail, DETAIL_MAX),
            raw,
            raw_sha256: hex(&sha256(stdin.as_bytes())),
            raw_len: stdin.len(),
        })
    }

    /// The event this request is for: absent means PreToolUse, a name this
    /// version doesn't know gives `None`.
    pub fn hook_event(&self) -> Option<HookEvent> {
        match &self.event {
            None => Some(HookEvent::PreToolUse),
            Some(name) => HookEvent::parse(name),
        }
    }

    /// The lifecycle notice for a request that isn't a PreToolUse.
    pub fn into_notice(self, event: HookEvent) -> HookNotice {
        HookNotice {
            event,
            job: self.job_id.map(JobId),
            session: self.session_id,
            cwd: self.cwd,
            tool: Some(self.tool_name).filter(|t| !t.is_empty()),
            summary: self.summary,
            detail: self.detail,
            input_sha256: self.raw_sha256,
            input_len: self.raw_len,
            raw: self.raw,
        }
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
            output: None,
        }
    }

    pub fn pass() -> WireReply {
        WireReply {
            decision: "pass".into(),
            reason: String::new(),
            output: None,
        }
    }

    /// A pass that makes the hook print `output`.
    pub fn print(output: String) -> WireReply {
        WireReply {
            output: Some(output),
            ..WireReply::pass()
        }
    }

    /// What the hook prints: Claude Code's PreToolUse JSON, or nothing to
    /// leave the call to Claude Code's own rules.
    pub fn hook_output(&self) -> Option<String> {
        if let Some(out) = &self.output {
            return Some(out.clone());
        }
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

fn str_at(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn first_line(s: &str) -> &str {
    s.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default()
        .trim()
}

/// `s` cut at `max` characters, with an ellipsis when cut.
fn cut(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
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
            ..WireRequest::default()
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
