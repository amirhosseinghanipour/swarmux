//! Parser for `claude -p --output-format stream-json` lines.
//!
//! Navigates `serde_json::Value` instead of strict structs, so new or unknown
//! fields never break parsing.

use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Init {
        session_id: String,
        model: String,
        cwd: String,
        permission_mode: String,
    },
    Text {
        text: String,
        parent: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        summary: String,
        parent: Option<String>,
    },
    ToolResult {
        tool_use_id: String,
        is_error: bool,
        preview: String,
        parent: Option<String>,
    },
    Usage {
        input: u64,
        output: u64,
        cache_read: u64,
        cache_creation: u64,
        parent: Option<String>,
    },
    Result {
        is_error: bool,
        subtype: String,
        cost_usd: f64,
        duration_ms: u64,
        num_turns: u32,
        tokens_in: u64,
        tokens_out: u64,
        cache_read: u64,
        context_window: Option<u64>,
        subagents_spawned: u32,
    },
    /// Known noise (hook progress, thinking token counts, rate-limit info, …).
    Ignored { kind: String },
    /// A type this version doesn't know yet; kept raw.
    Unknown { kind: String, raw: String },
    /// Not JSON at all (warnings, partial lines).
    Malformed { raw: String },
}

const SUMMARY_MAX: usize = 120;
const PREVIEW_MAX: usize = 200;

impl Event {
    pub fn kind(&self) -> &'static str {
        match self {
            Event::Init { .. } => "init",
            Event::Text { .. } => "text",
            Event::ToolUse { .. } => "tool_use",
            Event::ToolResult { .. } => "tool_result",
            Event::Usage { .. } => "usage",
            Event::Result { .. } => "result",
            Event::Ignored { .. } => "ignored",
            Event::Unknown { .. } => "unknown",
            Event::Malformed { .. } => "malformed",
        }
    }

    /// `"lead"` for the main session, or the parent `tool_use` id for subagent events.
    pub fn actor(&self) -> String {
        let parent = match self {
            Event::Text { parent, .. }
            | Event::ToolUse { parent, .. }
            | Event::ToolResult { parent, .. }
            | Event::Usage { parent, .. } => parent.as_deref(),
            _ => None,
        };
        parent.unwrap_or("lead").to_string()
    }
}

/// Parses one stdout line into zero or more events.
pub fn parse_line_all(line: &str) -> Vec<Event> {
    let line = line.trim();
    if line.is_empty() {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return vec![Event::Malformed {
            raw: line.to_string(),
        }];
    };
    let kind = str_at(&v, "type");
    match kind.as_str() {
        "system" => vec![parse_system(&v)],
        "assistant" => parse_assistant(&v),
        "user" => parse_user(&v),
        "result" => vec![parse_result(&v)],
        "rate_limit_event" | "stream_event" => vec![Event::Ignored { kind }],
        _ => vec![Event::Unknown {
            kind,
            raw: line.to_string(),
        }],
    }
}

fn parse_system(v: &Value) -> Event {
    let subtype = str_at(v, "subtype");
    if subtype == "init" {
        Event::Init {
            session_id: str_at(v, "session_id"),
            model: str_at(v, "model"),
            cwd: str_at(v, "cwd"),
            permission_mode: str_at(v, "permissionMode"),
        }
    } else {
        Event::Ignored {
            kind: format!("system/{subtype}"),
        }
    }
}

fn parent_of(v: &Value) -> Option<String> {
    v.get("parent_tool_use_id")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn content_blocks(v: &Value) -> &[Value] {
    v.pointer("/message/content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn parse_assistant(v: &Value) -> Vec<Event> {
    let parent = parent_of(v);
    let mut out = Vec::new();
    for b in content_blocks(v) {
        match b.get("type").and_then(Value::as_str) {
            Some("text") => out.push(Event::Text {
                text: str_at(b, "text"),
                parent: parent.clone(),
            }),
            Some("tool_use") => {
                let name = str_at(b, "name");
                out.push(Event::ToolUse {
                    id: str_at(b, "id"),
                    summary: tool_summary(&name, b.get("input").unwrap_or(&Value::Null)),
                    name,
                    parent: parent.clone(),
                });
            }
            _ => {} // thinking, redacted_thinking, …
        }
    }
    if let Some(u) = v.pointer("/message/usage") {
        out.push(Event::Usage {
            input: u64_at(u, "input_tokens"),
            output: u64_at(u, "output_tokens"),
            cache_read: u64_at(u, "cache_read_input_tokens"),
            cache_creation: u64_at(u, "cache_creation_input_tokens"),
            parent,
        });
    }
    out
}

fn parse_user(v: &Value) -> Vec<Event> {
    let parent = parent_of(v);
    content_blocks(v)
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
        .map(|b| Event::ToolResult {
            tool_use_id: str_at(b, "tool_use_id"),
            is_error: b.get("is_error").and_then(Value::as_bool).unwrap_or(false),
            preview: truncate(&result_text(b.get("content")), PREVIEW_MAX),
            parent: parent.clone(),
        })
        .collect()
}

fn parse_result(v: &Value) -> Event {
    let usage = v.get("usage").unwrap_or(&Value::Null);
    let context_window = v
        .get("modelUsage")
        .and_then(Value::as_object)
        .and_then(|m| {
            m.values()
                .filter_map(|x| x.get("contextWindow").and_then(Value::as_u64))
                .max()
        });
    Event::Result {
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

    fn fixture(name: &str) -> Vec<Event> {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .flat_map(parse_line_all)
            .collect()
    }

    fn kinds(events: &[Event]) -> Vec<&'static str> {
        events.iter().map(Event::kind).collect()
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
            Event::Init {
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
        let Some(Event::Result {
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
    fn subagent_events_have_parent_actor() {
        let ev = fixture("stream_subagent.jsonl");
        let task = &ev[1];
        assert_eq!(task.actor(), "lead");
        assert!(matches!(task, Event::ToolUse { summary, .. } if summary == "explorer: map code"));
        assert_eq!(ev[2].actor(), "toolu_A");
        assert_eq!(ev[3].actor(), "toolu_A");
        assert!(matches!(
            ev.last(),
            Some(Event::Result {
                subagents_spawned: 1,
                ..
            })
        ));
        assert!(
            matches!(&ev[4], Event::ToolResult { preview, .. } if preview == "src/main.rs has main")
        );
    }

    #[test]
    fn malformed_lines_do_not_stop_parsing() {
        assert_eq!(
            parse_line_all("not json"),
            [Event::Malformed {
                raw: "not json".into()
            }]
        );
        assert!(parse_line_all("").is_empty());
        assert!(parse_line_all("   ").is_empty());
    }

    #[test]
    fn unknown_type_is_kept_raw() {
        let line = r#"{"type":"brand_new","x":1}"#;
        assert_eq!(
            parse_line_all(line),
            [Event::Unknown {
                kind: "brand_new".into(),
                raw: line.into()
            }]
        );
    }

    #[test]
    fn bash_tool_summary_is_command() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t","name":"Bash","input":{"command":"cargo test"}}]}}"#;
        assert!(
            matches!(&parse_line_all(line)[0], Event::ToolUse { summary, .. } if summary == "cargo test")
        );
    }

    #[test]
    fn long_summaries_are_truncated() {
        let cmd = "x".repeat(500);
        let line = format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"t","name":"Bash","input":{{"command":"{cmd}"}}}}]}}}}"#
        );
        let Event::ToolUse { summary, .. } = &parse_line_all(&line)[0] else {
            panic!()
        };
        assert_eq!(summary.chars().count(), SUMMARY_MAX + 1);
    }
}
