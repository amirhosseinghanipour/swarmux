//! `claude agents --json`: the sessions Claude Code knows about, read-only.
//!
//! Interactive sessions carry `pid` and `status`; background ones carry
//! `id` and `state`; a blocked session may say what it `waitingFor`. The
//! shape isn't fixed, so unknown fields are ignored and missing ones left out.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;
use swarmux_application::AgentsPoll;
use swarmux_domain::telemetry::{ExternalKind, ExternalSession, ExternalState};

use super::cli;

/// How long `claude agents --json` may take.
const LIMIT: Duration = Duration::from_secs(20);

/// Runs `claude agents --json` (no shell, no console window).
pub struct ClaudeAgents {
    bin: PathBuf,
}

impl ClaudeAgents {
    pub fn new(bin: PathBuf) -> ClaudeAgents {
        ClaudeAgents { bin }
    }
}

impl AgentsPoll for ClaudeAgents {
    fn list(&self) -> Result<Vec<ExternalSession>, String> {
        let out = cli::run(&self.bin, &["agents", "--json"], None, LIMIT)
            .map_err(|e| format!("claude agents: {e}"))?;
        parse_agents(&out)
    }
}

/// Reads the array `claude agents --json` prints.
pub fn parse_agents(json: &str) -> Result<Vec<ExternalSession>, String> {
    let v: Value = serde_json::from_str(json.trim()).map_err(|e| format!("not JSON: {e}"))?;
    let rows = v.as_array().ok_or("not a list of sessions")?;
    Ok(rows.iter().filter_map(session).collect())
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn session(v: &Value) -> Option<ExternalSession> {
    let session_id = text(v, "sessionId").or_else(|| text(v, "id"))?;
    let kind = match text(v, "kind").as_deref() {
        Some("interactive") => ExternalKind::Interactive,
        Some("background") => ExternalKind::Background,
        _ => ExternalKind::Other,
    };
    let state = text(v, "state")
        .or_else(|| text(v, "status"))
        .map_or(ExternalState::Unknown, |s| ExternalState::parse(&s));
    Some(ExternalSession {
        name: text(v, "name").unwrap_or_else(|| session_id.chars().take(8).collect()),
        cwd: text(v, "cwd").unwrap_or_default(),
        kind,
        pid: v
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(|p| u32::try_from(p).ok()),
        state,
        waiting_for: v.get("waitingFor").and_then(waiting),
        started_at: v.get("startedAt").and_then(Value::as_i64),
        session_id,
    })
}

/// `waitingFor` as text: a string, an object with a `type` or `kind`, or a
/// list of those (the first is shown).
fn waiting(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        Value::Object(_) => ["type", "kind", "reason", "name"]
            .into_iter()
            .find_map(|k| text(v, k)),
        Value::Array(items) => items.iter().find_map(waiting),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The shape `claude agents --json` printed for one background and two
    // interactive sessions.
    const REAL: &str = r#"[
      {"id":"03873777","cwd":"C:\\Users\\u\\app","kind":"background","startedAt":1789837298113,
       "sessionId":"03873777-0976-4807-8873-0424728865f2","name":"Offline-first audit","state":"blocked"},
      {"pid":1632,"cwd":"C:\\Users\\u\\app","kind":"interactive","startedAt":1793546190748,
       "sessionId":"51d38a44-6409-4b0a-ba1c-6a5c85cfcfb8","name":"app-d2","status":"idle"},
      {"pid":22440,"cwd":"C:\\Users\\u","kind":"interactive","startedAt":1793634764493,
       "sessionId":"89722414-73c8-4b8a-80b0-68286b1e4435","name":"u-34","status":"busy"}
    ]"#;

    #[test]
    fn the_real_shape() {
        let s = parse_agents(REAL).unwrap();
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].kind, ExternalKind::Background);
        assert_eq!(s[0].state, ExternalState::Blocked);
        assert!(s[0].is_waiting());
        assert_eq!(s[0].waiting_for, None);
        assert_eq!(s[1].state, ExternalState::Idle);
        assert_eq!(s[1].pid, Some(1632));
        assert_eq!(s[2].state, ExternalState::Working);
        assert_eq!(s[2].started_at, Some(1_793_634_764_493));
        assert_eq!(s[2].name, "u-34");
    }

    #[test]
    fn waiting_for_in_its_possible_forms() {
        let rows = r#"[
          {"sessionId":"a","state":"blocked","waitingFor":"permission prompt"},
          {"sessionId":"b","state":"blocked","waitingFor":{"type":"input needed"}},
          {"sessionId":"c","status":"busy","waitingFor":[{"kind":"sandbox request"}]},
          {"sessionId":"d","state":"working","waitingFor":null}
        ]"#;
        let s = parse_agents(rows).unwrap();
        assert_eq!(s[0].waiting_for.as_deref(), Some("permission prompt"));
        assert_eq!(s[1].waiting_for.as_deref(), Some("input needed"));
        assert_eq!(s[2].waiting_for.as_deref(), Some("sandbox request"));
        assert!(s[2].is_waiting(), "a reason means waiting even when busy");
        assert!(!s[3].is_waiting());
    }

    #[test]
    fn rows_without_an_id_and_bad_output() {
        assert_eq!(
            parse_agents(r#"[{"name":"x"},{"id":"1"}]"#).unwrap().len(),
            1
        );
        assert!(parse_agents("not json").is_err());
        assert!(parse_agents(r#"{"a":1}"#).is_err());
        assert!(parse_agents("[]").unwrap().is_empty());
    }
}
