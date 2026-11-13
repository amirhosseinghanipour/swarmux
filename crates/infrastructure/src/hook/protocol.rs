//! What travels between `swarmux hook pre-tool` and the running app, and
//! what the hook prints for Claude Code.
//!
//! Both directions are one line of JSON.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use swarmux_application::ports::HookRequest;
use swarmux_application::{BoardReply, HookNotice};
use swarmux_domain::ledger::{hex, sha256};
use swarmux_domain::telemetry::InstructionFile;
use swarmux_domain::{Decision, HookEvent, JobId, Verdict};

/// Longest tool input kept for display, in characters.
const INPUT_MAX: usize = 2000;
/// Longest hook input carried over the socket, in bytes.
pub const RAW_MAX: usize = 16 * 1024;
/// Longest one-line summary, in characters.
const SUMMARY_MAX: usize = 200;
/// Longest detail text (a last message), in characters.
const DETAIL_MAX: usize = 4000;

/// The `event` of a request that carries a status line payload.
pub const STATUSLINE_EVENT: &str = "Statusline";

/// The `event` of a request that carries a board tool call: `tool_name` is
/// the tool and `tool_input` its arguments.
pub const BOARD_EVENT: &str = "Board";

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
    /// The subagent that made the call, when Claude Code says which.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    /// What the hook does when it can't reach the app (`closed`, `ask` or
    /// `open`), so an app that goes away mid-call answers the same way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail: Option<String>,
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
    /// Text a PreToolUse answer adds to the agent's context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
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
            agent_id: opt_str_at(&v, "agent_id"),
            agent_type: opt_str_at(&v, "agent_type"),
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
            HookEvent::InstructionsLoaded => (
                str_at(&v, "file_path"),
                InstructionFile::detail(&str_at(&v, "load_reason"), &str_at(&v, "memory_type")),
            ),
            HookEvent::TaskCreated | HookEvent::TaskCompleted => {
                (str_at(&v, "task_subject"), str_at(&v, "task_description"))
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
            agent_id: opt_str_at(&v, "agent_id"),
            agent_type: opt_str_at(&v, "agent_type"),
            fail: None,
        })
    }

    /// The request that carries a status line payload, whole, in `raw`.
    pub fn statusline(payload: &str, job_id: Option<i64>) -> WireRequest {
        WireRequest {
            job_id,
            event: Some(STATUSLINE_EVENT.to_string()),
            raw: payload.to_string(),
            ..WireRequest::default()
        }
    }

    /// A call of one of the board's MCP tools.
    pub fn board(tool: &str, arguments: Value, job_id: Option<i64>) -> WireRequest {
        WireRequest {
            job_id,
            tool_name: tool.to_string(),
            tool_input: arguments,
            event: Some(BOARD_EVENT.to_string()),
            ..WireRequest::default()
        }
    }

    pub fn is_board(&self) -> bool {
        self.event.as_deref() == Some(BOARD_EVENT)
    }

    /// The request carries a status line payload.
    pub fn is_statusline(&self) -> bool {
        self.event.as_deref() == Some(STATUSLINE_EVENT)
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
        let content = written_text(&self.tool_name, &self.tool_input);
        HookRequest {
            request_id,
            job_id: self.job_id.map(JobId),
            tool_name: self.tool_name,
            subject,
            input,
            cwd: self.cwd,
            content,
            agent_id: self.agent_id,
            agent_type: self.agent_type,
        }
    }
}

/// Longest text a file tool writes that knowledge rules look at, in bytes.
const CONTENT_MAX: usize = 256 * 1024;

/// What a file tool writes: `content`, `new_string`, or every edit's
/// `new_string` (MultiEdit), joined; empty for other tools.
pub fn written_text(tool: &str, input: &Value) -> String {
    let field = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let mut out = match tool {
        "Write" => field(input, "content").unwrap_or_default(),
        "Edit" => field(input, "new_string").unwrap_or_default(),
        "MultiEdit" => input
            .get("edits")
            .and_then(Value::as_array)
            .map(|edits| {
                edits
                    .iter()
                    .filter_map(|e| field(e, "new_string"))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default(),
        "NotebookEdit" => field(input, "new_source").unwrap_or_default(),
        _ => String::new(),
    };
    if out.len() > CONTENT_MAX {
        let mut end = CONTENT_MAX;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    out
}

impl WireReply {
    pub fn from_decision(d: &Decision) -> WireReply {
        let decision = match d.verdict {
            Verdict::Allow => "allow",
            Verdict::Deny => "deny",
            // The app never forwards "ask": in `claude -p` it would mean deny.
            Verdict::Ask | Verdict::Pass => "pass",
        };
        let mut reason = match (&d.rule, d.verdict) {
            (Some(rule), Verdict::Allow) => format!("swarmux guard allowed it: {rule}"),
            (Some(rule), Verdict::Deny) => format!("swarmux guard denied it: {rule}"),
            (None, Verdict::Deny) => "swarmux guard denied it".to_string(),
            _ => String::new(),
        };
        // The user's words ride along with a denial: Claude Code shows the
        // reason to the agent, which can then change what it does.
        if let (Some(note), Verdict::Deny) = (&d.note, d.verdict) {
            reason.push_str(&format!(". The operator's note: {note}"));
        }
        WireReply {
            decision: decision.into(),
            reason,
            output: None,
            context: None,
        }
    }

    /// The answer to a board call.
    pub fn board(reply: &BoardReply) -> WireReply {
        WireReply {
            decision: if reply.error { "error" } else { "ok" }.into(),
            reason: String::new(),
            output: Some(reply.text.clone()),
            context: None,
        }
    }

    /// Reads the answer to a board call.
    pub fn into_board(self) -> BoardReply {
        BoardReply {
            text: self.output.unwrap_or_default(),
            error: self.decision != "ok",
        }
    }

    /// Leave the call to Claude Code's own question.
    pub fn ask(reason: &str) -> WireReply {
        WireReply {
            decision: "ask".into(),
            reason: reason.to_string(),
            output: None,
            context: None,
        }
    }

    pub fn pass() -> WireReply {
        WireReply {
            decision: "pass".into(),
            reason: String::new(),
            output: None,
            context: None,
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
    /// leave the call to Claude Code's own rules. Context rides along with
    /// any decision; with none, it is added and the call left to Claude Code.
    pub fn hook_output(&self) -> Option<String> {
        if let Some(out) = &self.output {
            return Some(out.clone());
        }
        let decision = match self.decision.as_str() {
            "allow" => Some("allow"),
            "deny" => Some("deny"),
            "ask" => Some("ask"),
            _ => None,
        };
        let mut out = json!({ "hookEventName": "PreToolUse" });
        if let Some(d) = decision {
            out["permissionDecision"] = d.into();
            out["permissionDecisionReason"] = self.reason.clone().into();
        }
        match &self.context {
            Some(text) => out["additionalContext"] = text.clone().into(),
            None if decision.is_none() => return None,
            None => {}
        }
        Some(json!({ "hookSpecificOutput": out }).to_string())
    }
}

fn str_at(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn opt_str_at(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
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
    fn context_rides_along_with_or_without_a_decision() {
        let mut pass = WireReply::pass();
        assert_eq!(pass.hook_output(), None);
        pass.context = Some("SWARMUX KNOWLEDGE".into());
        let v: Value = serde_json::from_str(&pass.hook_output().unwrap()).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(
            v["hookSpecificOutput"]["additionalContext"],
            "SWARMUX KNOWLEDGE"
        );
        assert!(v["hookSpecificOutput"].get("permissionDecision").is_none());
        let mut allow = WireReply::from_decision(&Decision::new(Verdict::Allow, None));
        allow.context = Some("c".into());
        let v: Value = serde_json::from_str(&allow.hook_output().unwrap()).unwrap();
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "allow");
        assert_eq!(v["hookSpecificOutput"]["additionalContext"], "c");
        let wire: WireReply =
            serde_json::from_str(&serde_json::to_string(&allow).unwrap()).unwrap();
        assert_eq!(wire, allow);
    }

    #[test]
    fn written_text_per_tool() {
        let w = |tool: &str, v: Value| written_text(tool, &v);
        assert_eq!(w("Write", json!({"file_path": "a", "content": "x"})), "x");
        assert_eq!(
            w("Edit", json!({"old_string": "o", "new_string": "n"})),
            "n"
        );
        assert_eq!(
            w(
                "MultiEdit",
                json!({"edits": [{"new_string": "a"}, {"new_string": "b"}]})
            ),
            "a\nb"
        );
        assert_eq!(w("Bash", json!({"command": "ls"})), "");
        let big = w("Write", json!({"content": "é".repeat(CONTENT_MAX)}));
        assert!(big.len() <= CONTENT_MAX);
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

    #[test]
    fn a_denial_carries_the_operators_note_to_the_agent() {
        let note = "use the staging bucket, not prod";
        let deny = WireReply::from_decision(
            &Decision::new(Verdict::Deny, Some("denied in swarmux".into())).with_note(note),
        );
        let v: Value = serde_json::from_str(&deny.hook_output().unwrap()).unwrap();
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(
            v["hookSpecificOutput"]["permissionDecisionReason"],
            "swarmux guard denied it: denied in swarmux. The operator's note: use the staging bucket, not prod"
        );
        // With no rule the note still arrives.
        let bare = WireReply::from_decision(&Decision::new(Verdict::Deny, None).with_note(note));
        assert!(bare.reason.ends_with(note), "{}", bare.reason);
        // A blank note is no note; an allow has none to carry.
        let blank = WireReply::from_decision(&Decision::new(Verdict::Deny, None).with_note("  "));
        assert_eq!(blank.reason, "swarmux guard denied it");
        let allow = WireReply::from_decision(
            &Decision::new(Verdict::Allow, Some("ok".into())).with_note(note),
        );
        assert_eq!(allow.reason, "swarmux guard allowed it: ok");
    }
}
