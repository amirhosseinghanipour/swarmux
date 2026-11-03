//! Translates `claude -p --output-format stream-json` lines into domain events.
//!
//! Navigates `serde_json::Value` instead of strict structs, so new or unknown
//! fields never break parsing.

use serde_json::Value;
use swarmux_domain::telemetry::{Environment, McpServer, RateLimit, parse_todos};
use swarmux_domain::{Actor, AgentEvent, SessionState};

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
        "system" => parse_system(&v),
        "assistant" => parse_assistant(&v),
        "user" => parse_user(&v),
        "result" => vec![parse_result(&v)],
        "rate_limit_event" => parse_rate_limits(&v),
        "stream_event" => vec![AgentEvent::Ignored { kind }],
        _ => vec![AgentEvent::Unknown {
            kind,
            raw: line.to_string(),
        }],
    }
}

fn parse_system(v: &Value) -> Vec<AgentEvent> {
    let subtype = str_at(v, "subtype");
    if subtype == "session_state_changed" {
        return match SessionState::parse(&str_at(v, "state")) {
            Some(state) => vec![AgentEvent::SessionState(state)],
            None => vec![AgentEvent::Ignored {
                kind: format!("system/{subtype}"),
            }],
        };
    }
    if subtype == "init" {
        vec![
            AgentEvent::Init {
                session_id: str_at(v, "session_id"),
                model: str_at(v, "model"),
                cwd: str_at(v, "cwd"),
                permission_mode: str_at(v, "permissionMode"),
            },
            AgentEvent::Environment(Box::new(parse_environment(v))),
        ]
    } else {
        vec![AgentEvent::Ignored {
            kind: format!("system/{subtype}"),
        }]
    }
}

/// The session's tools, servers, plugins, skills and agents from `system/init`.
fn parse_environment(v: &Value) -> Environment {
    let names = |key: &str| -> Vec<String> {
        v.get(key)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|x| {
                        x.as_str()
                            .or_else(|| x.get("name").and_then(Value::as_str))
                            .map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let opt = |key: &str| Some(str_at(v, key)).filter(|s| !s.is_empty());
    let mcp_servers = v
        .get("mcp_servers")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|x| McpServer {
                    name: str_at(x, "name"),
                    status: str_at(x, "status"),
                })
                .filter(|s| !s.name.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Environment {
        session_id: str_at(v, "session_id"),
        claude_code_version: opt("claude_code_version"),
        model: str_at(v, "model"),
        permission_mode: str_at(v, "permissionMode"),
        output_style: opt("output_style"),
        tools: names("tools"),
        mcp_servers,
        plugins: names("plugins"),
        skills: names("skills"),
        agents: names("agents"),
    }
}

/// One reading per limit window: every entry of `unifiedWindows`, else the
/// single `rateLimitType` window. A bare status is ignored.
fn parse_rate_limits(v: &Value) -> Vec<AgentEvent> {
    let ignored = || {
        vec![AgentEvent::Ignored {
            kind: "rate_limit_event".into(),
        }]
    };
    let Some(info) = v.get("rate_limit_info") else {
        return ignored();
    };
    let reading = |window: &str, x: &Value| RateLimit {
        window: window.to_string(),
        status: str_at(x, "status"),
        utilization: x.get("utilization").and_then(Value::as_f64),
        // Claude Code reports seconds since the epoch.
        resets_at: x
            .get("resetsAt")
            .and_then(Value::as_f64)
            .map(|s| (s * 1000.0) as i64),
        surpassed_threshold: x.get("surpassedThreshold").and_then(Value::as_f64),
    };
    let windows: Vec<AgentEvent> = info
        .get("unifiedWindows")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(w, x)| AgentEvent::RateLimit(reading(w, x)))
                .collect()
        })
        .unwrap_or_default();
    if !windows.is_empty() {
        return windows;
    }
    let window = str_at(info, "rateLimitType");
    if window.is_empty() {
        return ignored();
    }
    vec![AgentEvent::RateLimit(reading(&window, info))]
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
                let input = b.get("input").unwrap_or(&Value::Null);
                let todos = (name == "TodoWrite")
                    .then(|| parse_todos(&input.to_string()))
                    .flatten();
                out.push(AgentEvent::ToolUse {
                    id: str_at(b, "id"),
                    summary: tool_summary(&name, input),
                    name,
                    actor: actor.clone(),
                });
                if let Some(todos) = todos {
                    out.push(AgentEvent::Todos {
                        todos,
                        actor: actor.clone(),
                    });
                }
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
        text: str_at(v, "result"),
    }
}

fn tool_summary(name: &str, input: &Value) -> String {
    let s = match name {
        "Bash" | "PowerShell" => str_at(input, "command"),
        "Read" | "Edit" | "Write" | "MultiEdit" => str_at(input, "file_path"),
        "NotebookEdit" | "NotebookRead" => {
            let path = str_at(input, "notebook_path");
            if path.is_empty() {
                str_at(input, "file_path")
            } else {
                path
            }
        }
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
                "environment",
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
        assert!(matches!(ev.last(), Some(AgentEvent::Result { text, .. }) if !text.is_empty()));
        assert!(!is_error);
    }

    #[test]
    fn subagent_events_have_subagent_actor() {
        let ev: Vec<AgentEvent> = fixture("stream_subagent.jsonl")
            .into_iter()
            .filter(|e| e.kind() != "environment")
            .collect();
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

    /// Shaped like Claude Code 2.1's `rate_limit_event` (values made up).
    const RATE_LIMIT: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","resetsAt":1793786400,"rateLimitType":"seven_day","utilization":0.83,"overageStatus":"rejected","isUsingOverage":false,"surpassedThreshold":0.75,"unifiedWindows":{"seven_day":{"utilization":0.83,"resetsAt":1793786400,"status":"allowed_warning","surpassedThreshold":0.75},"five_hour":{"utilization":0.12,"resetsAt":1793700000,"status":"allowed"}}},"uuid":"u","session_id":"s"}"#;

    #[test]
    fn rate_limit_event_gives_one_reading_per_window() {
        let ev = parse_line(RATE_LIMIT);
        assert_eq!(ev.len(), 2);
        let week = ev.iter().find_map(|e| match e {
            AgentEvent::RateLimit(l) if l.window == "seven_day" => Some(l),
            _ => None,
        });
        assert_eq!(
            week,
            Some(&RateLimit {
                window: "seven_day".into(),
                status: "allowed_warning".into(),
                utilization: Some(0.83),
                resets_at: Some(1_793_786_400_000),
                surpassed_threshold: Some(0.75),
            })
        );
    }

    #[test]
    fn rate_limit_without_windows() {
        let single = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","rateLimitType":"five_hour","resetsAt":100,"utilization":0.5}}"#;
        let ev = parse_line(single);
        assert!(
            matches!(&ev[..], [AgentEvent::RateLimit(l)] if l.window == "five_hour" && l.resets_at == Some(100_000)),
            "{ev:?}"
        );
        let bare = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#;
        assert_eq!(parse_line(bare)[0].kind(), "ignored");
    }

    /// Shaped like Claude Code 2.1's `system/init` (names made up, lists shortened).
    const INIT: &str = r#"{"type":"system","subtype":"init","cwd":"/work/demo","session_id":"00000000-0000-4000-8000-000000000009","tools":["Task","Bash","Read"],"mcp_servers":[{"name":"docs","status":"connected","source":"claudeai"},{"name":"mail","status":"needs-auth","source":"claudeai"},{"name":"local","status":"pending","source":"user"}],"model":"claude-haiku-5-5","permissionMode":"default","slash_commands":["review"],"apiKeySource":"none","claude_code_version":"2.1.316","output_style":"default","agents":["claude","explorer"],"skills":["pdf","docx"],"plugins":[{"name":"scholar","path":"/p/scholar","source":"scholar@scholar","version":"1.0.0"}],"capabilities":["x"],"uuid":"u"}"#;

    /// As Claude Code 2.1 prints it with `CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS=1`.
    #[test]
    fn session_state_changes_become_events() {
        let line = |s: &str| {
            format!(
                r#"{{"type":"system","subtype":"session_state_changed","state":"{s}","uuid":"u","session_id":"s"}}"#
            )
        };
        assert_eq!(
            parse_line(&line("running")),
            [AgentEvent::SessionState(SessionState::Running)]
        );
        assert_eq!(
            parse_line(&line("requires_action")),
            [AgentEvent::SessionState(SessionState::RequiresAction)]
        );
        assert_eq!(
            parse_line(&line("idle")),
            [AgentEvent::SessionState(SessionState::Idle)]
        );
        assert!(matches!(
            parse_line(&line("teleporting")).as_slice(),
            [AgentEvent::Ignored { .. }]
        ));
    }

    #[test]
    fn init_also_reports_the_environment() {
        let ev = parse_line(INIT);
        assert_eq!(kinds(&ev), ["init", "environment"]);
        let AgentEvent::Environment(env) = &ev[1] else {
            panic!("{ev:?}")
        };
        assert_eq!(env.session_id, "00000000-0000-4000-8000-000000000009");
        assert_eq!(env.claude_code_version.as_deref(), Some("2.1.316"));
        assert_eq!(env.output_style.as_deref(), Some("default"));
        assert_eq!(env.tools, ["Task", "Bash", "Read"]);
        assert_eq!(env.mcp_up(), (1, 3));
        assert_eq!(env.plugins, ["scholar"]);
        assert_eq!((env.skills.len(), env.agents.len()), (2, 2));
    }

    #[test]
    fn todo_write_also_reports_the_todos() {
        let line = r#"{"type":"assistant","parent_tool_use_id":"toolu_A","message":{"content":[{"type":"tool_use","id":"t","name":"TodoWrite","input":{"todos":[{"content":"Map code","status":"in_progress","activeForm":"Mapping code"},{"content":"Report","status":"pending","activeForm":"Reporting"}]}}]}}"#;
        let ev = parse_line(line);
        assert_eq!(kinds(&ev), ["tool_use", "todos"]);
        let AgentEvent::Todos { todos, actor } = &ev[1] else {
            panic!("{ev:?}")
        };
        assert_eq!(actor, &Actor::Subagent("toolu_A".into()));
        assert_eq!(todos[0].active_form.as_deref(), Some("Mapping code"));
        assert_eq!(todos.len(), 2);
    }

    #[test]
    fn file_tools_summarize_to_their_path() {
        let call = |name: &str, input: &str| {
            let line = format!(
                r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"t","name":"{name}","input":{input}}}]}}}}"#
            );
            match &parse_line(&line)[0] {
                AgentEvent::ToolUse { summary, .. } => summary.clone(),
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(
            call("MultiEdit", r#"{"file_path":"src/a.rs","edits":[]}"#),
            "src/a.rs"
        );
        assert_eq!(
            call("NotebookEdit", r#"{"notebook_path":"n.ipynb"}"#),
            "n.ipynb"
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
