//! Translates `claude -p --output-format stream-json` lines into domain events.
//!
//! Navigates `serde_json::Value` instead of strict structs, so new or unknown
//! fields never break parsing.

use serde_json::Value;
use swarmux_domain::{Actor, AgentEvent};

const SUMMARY_MAX: usize = 120;
const PREVIEW_MAX: usize = 200;

/// Parses one stdout line into zero or more events.
pub fn parse_line(line: &str) -> Vec<AgentEvent> {
    let line = line.trim();
    if line.is_empty() {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return vec![AgentEvent::Malformed {
            raw: line.to_string(),
        }];
    };
    let kind = str_at(&v, "type");
    match kind.as_str() {
        "system" => vec![parse_system(&v)],
        "assistant" => parse_assistant(&v),
        "user" => parse_user(&v),
        "result" => vec![parse_result(&v)],
        "rate_limit_event" | "stream_event" => vec![AgentEvent::Ignored { kind }],
        _ => vec![AgentEvent::Unknown {
            kind,
            raw: line.to_string(),
        }],
    }
}

fn parse_system(v: &Value) -> AgentEvent {
    let subtype = str_at(v, "subtype");
    if subtype == "init" {
        AgentEvent::Init {
            session_id: str_at(v, "session_id"),
            model: str_at(v, "model"),
            cwd: str_at(v, "cwd"),
            permission_mode: str_at(v, "permissionMode"),
        }
    } else {
        AgentEvent::Ignored {
            kind: format!("system/{subtype}"),
        }
    }
}

fn actor_of(v: &Value) -> Actor {
    match v.get("parent_tool_use_id").and_then(Value::as_str) {
        Some(id) => Actor::Subagent(id.to_string()),
        None => Actor::Lead,
    }
}

fn content_blocks(v: &Value) -> &[Value] {
    v.pointer("/message/content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn parse_assistant(v: &Value) -> Vec<AgentEvent> {
    let actor = actor_of(v);
    let mut out = Vec::new();
    for b in content_blocks(v) {
        match b.get("type").and_then(Value::as_str) {
            Some("text") => out.push(AgentEvent::Text {
                text: str_at(b, "text"),
                actor: actor.clone(),
            }),
            Some("tool_use") => {
                let name = str_at(b, "name");
                out.push(AgentEvent::ToolUse {
                    id: str_at(b, "id"),
                    summary: tool_summary(&name, b.get("input").unwrap_or(&Value::Null)),
                    name,
                    actor: actor.clone(),
                });
            }
            _ => {} // thinking, redacted_thinking, …
        }
    }
    if let Some(u) = v.pointer("/message/usage") {
        out.push(AgentEvent::Usage {
            input: u64_at(u, "input_tokens"),
            output: u64_at(u, "output_tokens"),
            cache_read: u64_at(u, "cache_read_input_tokens"),
            cache_creation: u64_at(u, "cache_creation_input_tokens"),
            actor,
        });
    }
    out
}

fn parse_user(v: &Value) -> Vec<AgentEvent> {
    let actor = actor_of(v);
    content_blocks(v)
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
        .map(|b| AgentEvent::ToolResult {
            tool_use_id: str_at(b, "tool_use_id"),
            is_error: b.get("is_error").and_then(Value::as_bool).unwrap_or(false),
            preview: truncate(&result_text(b.get("content")), PREVIEW_MAX),
            actor: actor.clone(),
        })
        .collect()
}

fn parse_result(v: &Value) -> AgentEvent {
    let usage = v.get("usage").unwrap_or(&Value::Null);
    let context_window = v
        .get("modelUsage")
        .and_then(Value::as_object)
        .and_then(|m| {
            m.values()
                .filter_map(|x| x.get("contextWindow").and_then(Value::as_u64))
                .max()
        });
    AgentEvent::Result {
        is_error: v.get("is_error").and_then(Value::as_bool).unwrap_or(false),
        subtype: str_at(v, "subtype"),
        cost_usd: v
            .get("total_cost_usd")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        duration_ms: u64_at(v, "duration_ms"),
        num_turns: u64_at(v, "num_turns") as u32,
        tokens_in: u64_at(usage, "input_tokens"),
        tokens_out: u64_at(usage, "output_tokens"),
        cache_read: u64_at(usage, "cache_read_input_tokens"),
        context_window,
        subagents_spawned: v
            .pointer("/subagent_stats/spawned")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
    }
}

fn tool_summary(name: &str, input: &Value) -> String {
    let s = match name {
        "Bash" | "PowerShell" => str_at(input, "command"),
        "Read" | "Edit" | "Write" | "NotebookEdit" => str_at(input, "file_path"),
        "Task" | "Agent" => format!(
            "{}: {}",
            str_at(input, "subagent_type"),
            str_at(input, "description")
        ),
        "Grep" | "Glob" => str_at(input, "pattern"),
        _ => String::new(),
    };
    let s = if s.trim().is_empty() {
        name.to_string()
    } else {
        s
    };
    truncate(&s, SUMMARY_MAX)
}

fn result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn str_at(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn u64_at(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn truncate(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<AgentEvent> {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .flat_map(parse_line)
            .collect()
    }

    fn kinds(events: &[AgentEvent]) -> Vec<&'static str> {
        events.iter().map(AgentEvent::kind).collect()
    }

    #[test]
    fn parses_basic_fixture_in_order() {
        let ev = fixture("stream_basic.jsonl");
        assert_eq!(
            kinds(&ev),
            [
                "ignored",
                "init",
                "tool_use",
                "usage",
                "ignored",
                "tool_result",
                "text",
                "usage",
                "malformed",
                "result"
            ]
        );
        assert_eq!(
            ev[1],
            AgentEvent::Init {
                session_id: "00000000-0000-4000-8000-000000000001".into(),
                model: "claude-haiku-5-5".into(),
                cwd: "/work/demo".into(),
                permission_mode: "default".into(),
            }
        );
    }

    #[test]
    fn result_carries_cost_and_context_window() {
        let ev = fixture("stream_basic.jsonl");
        let Some(AgentEvent::Result {
            cost_usd,
            duration_ms,
            num_turns,
            context_window,
            tokens_out,
            is_error,
            ..
        }) = ev.last().cloned()
        else {
            panic!("last event is not a result");
        };
        assert_eq!(cost_usd, 0.0108);
        assert_eq!(duration_ms, 44116);
        assert_eq!(num_turns, 2);
        assert_eq!(context_window, Some(1_000_000));
        assert_eq!(tokens_out, 385);
        assert!(!is_error);
    }

    #[test]
    fn subagent_events_have_subagent_actor() {
        let ev = fixture("stream_subagent.jsonl");
        assert_eq!(ev[1].actor(), &Actor::Lead);
        assert!(
            matches!(&ev[1], AgentEvent::ToolUse { summary, .. } if summary == "explorer: map code")
        );
        assert_eq!(ev[2].actor(), &Actor::Subagent("toolu_A".into()));
        assert_eq!(ev[3].actor(), &Actor::Subagent("toolu_A".into()));
        assert!(matches!(
            ev.last(),
            Some(AgentEvent::Result {
                subagents_spawned: 1,
                ..
            })
        ));
        assert!(
            matches!(&ev[4], AgentEvent::ToolResult { preview, .. } if preview == "src/main.rs has main")
        );
    }

    #[test]
    fn malformed_lines_do_not_stop_parsing() {
        assert_eq!(
            parse_line("not json"),
            [AgentEvent::Malformed {
                raw: "not json".into()
            }]
        );
        assert!(parse_line("").is_empty());
        assert!(parse_line("   ").is_empty());
    }

    #[test]
    fn unknown_type_is_kept_raw() {
        let line = r#"{"type":"brand_new","x":1}"#;
        assert_eq!(
            parse_line(line),
            [AgentEvent::Unknown {
                kind: "brand_new".into(),
                raw: line.into()
            }]
        );
    }

    #[test]
    fn bash_tool_summary_is_command() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t","name":"Bash","input":{"command":"cargo test"}}]}}"#;
        assert!(
            matches!(&parse_line(line)[0], AgentEvent::ToolUse { summary, .. } if summary == "cargo test")
        );
    }

    #[test]
    fn long_summaries_are_truncated() {
        let cmd = "x".repeat(500);
        let line = format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"t","name":"Bash","input":{{"command":"{cmd}"}}}}]}}}}"#
        );
        let AgentEvent::ToolUse { summary, .. } = &parse_line(&line)[0] else {
            panic!()
        };
        assert_eq!(summary.chars().count(), SUMMARY_MAX + 1);
    }
}
