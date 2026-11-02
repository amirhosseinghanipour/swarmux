//! Subagents of a job, followed from the Task/Agent call that starts them to
//! the tool result that ends them.

use crate::{Actor, AgentEvent, TokenTally};

const STATUS_CHARS: usize = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubagentStatus {
    Running,
    Done,
    Failed,
}

/// The latest tool result a subagent got.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolOutcome {
    pub tool: String,
    /// First line of the result, shortened.
    pub text: String,
    pub is_error: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Subagent {
    /// The tool_use id of the call that started it.
    pub id: String,
    /// `subagent_type`, e.g. "explorer".
    pub kind: String,
    pub description: String,
    /// From the agent's definition file, when known.
    pub model: Option<String>,
    pub started_tick: u32,
    pub ended_tick: Option<u32>,
    pub status: SubagentStatus,
    pub tokens: TokenTally,
    pub tool_calls: u32,
    /// Ticks at which it reported anything.
    pub marks: Vec<u32>,
    pub last_outcome: Option<ToolOutcome>,
    /// First line of what it handed back to the lead.
    pub result: Option<String>,
    /// Its own tool calls still waiting for a result: (id, tool).
    open_calls: Vec<(String, String)>,
}

impl Subagent {
    fn start(id: &str, summary: &str, name: &str, tick: u32) -> Subagent {
        let (kind, description) = match summary.split_once(':') {
            Some((k, d)) if !k.trim().is_empty() => (k.trim(), d.trim()),
            _ => (name, summary.trim()),
        };
        Subagent {
            id: id.to_string(),
            kind: kind.to_string(),
            description: description.to_string(),
            model: None,
            started_tick: tick,
            ended_tick: None,
            status: SubagentStatus::Running,
            tokens: TokenTally::default(),
            tool_calls: 0,
            marks: Vec::new(),
            last_outcome: None,
            result: None,
            open_calls: Vec::new(),
        }
    }

    pub fn is_running(&self) -> bool {
        self.status == SubagentStatus::Running
    }

    /// Output tokens per second over its run.
    pub fn rate(&self) -> Option<f64> {
        self.tokens.rate(Some(self.started_tick))
    }
}

/// Updates `subagents` with one event at `tick`.
pub fn observe(subagents: &mut Vec<Subagent>, ev: &AgentEvent, tick: u32) {
    if let AgentEvent::ToolUse {
        id, name, summary, ..
    } = ev
        && ev.starts_subagent()
        && !subagents.iter().any(|s| &s.id == id)
    {
        subagents.push(Subagent::start(id, summary, name, tick));
        return;
    }
    if let AgentEvent::ToolResult {
        tool_use_id,
        is_error,
        preview,
        actor: Actor::Lead,
    } = ev
        && let Some(s) = subagents.iter_mut().find(|s| &s.id == tool_use_id)
    {
        s.status = if *is_error {
            SubagentStatus::Failed
        } else {
            SubagentStatus::Done
        };
        s.ended_tick = Some(tick);
        s.result = Some(first_line(preview));
        return;
    }
    let Actor::Subagent(owner) = ev.actor() else {
        return;
    };
    let Some(s) = subagents.iter_mut().find(|s| &s.id == owner) else {
        return;
    };
    s.marks.push(tick);
    match ev {
        AgentEvent::ToolUse { id, name, .. } => {
            s.tool_calls += 1;
            s.open_calls.push((id.clone(), name.clone()));
        }
        AgentEvent::ToolResult {
            tool_use_id,
            is_error,
            preview,
            ..
        } => {
            let tool = match s.open_calls.iter().position(|(id, _)| id == tool_use_id) {
                Some(i) => s.open_calls.remove(i).1,
                None => "tool".to_string(),
            };
            s.last_outcome = Some(ToolOutcome {
                tool,
                text: first_line(preview),
                is_error: *is_error,
            });
        }
        AgentEvent::Usage {
            input,
            output,
            cache_read,
            cache_creation,
            ..
        } => s
            .tokens
            .add(*input, *output, *cache_read, *cache_creation, tick),
        _ => {}
    }
}

fn first_line(s: &str) -> String {
    let line = s.trim().lines().next().unwrap_or_default().trim();
    match line.char_indices().nth(STATUS_CHARS) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, summary: &str) -> AgentEvent {
        AgentEvent::ToolUse {
            id: id.into(),
            name: "Task".into(),
            summary: summary.into(),
            actor: Actor::Lead,
        }
    }

    fn sub(id: &str) -> Actor {
        Actor::Subagent(id.into())
    }

    #[test]
    fn follows_one_subagent_to_the_end() {
        let mut list = Vec::new();
        observe(&mut list, &task("A", "worker: run the tests"), 3);
        assert_eq!(
            (list[0].kind.as_str(), list[0].description.as_str()),
            ("worker", "run the tests")
        );
        observe(
            &mut list,
            &AgentEvent::ToolUse {
                id: "b1".into(),
                name: "Bash".into(),
                summary: "pytest".into(),
                actor: sub("A"),
            },
            5,
        );
        observe(
            &mut list,
            &AgentEvent::ToolResult {
                tool_use_id: "b1".into(),
                is_error: false,
                preview: "22 passed, 6 failed\nmore".into(),
                actor: sub("A"),
            },
            9,
        );
        observe(
            &mut list,
            &AgentEvent::Usage {
                input: 10,
                output: 40,
                cache_read: 0,
                cache_creation: 0,
                actor: sub("A"),
            },
            23,
        );
        let s = &list[0];
        assert!(s.is_running());
        assert_eq!(s.tool_calls, 1);
        assert_eq!(
            s.last_outcome,
            Some(ToolOutcome {
                tool: "Bash".into(),
                text: "22 passed, 6 failed".into(),
                is_error: false
            })
        );
        assert_eq!(s.marks, [5, 9, 23]);
        assert_eq!(s.rate(), Some(20.0));

        observe(
            &mut list,
            &AgentEvent::ToolResult {
                tool_use_id: "A".into(),
                is_error: false,
                preview: "all green".into(),
                actor: Actor::Lead,
            },
            30,
        );
        assert_eq!(list[0].status, SubagentStatus::Done);
        assert_eq!(list[0].ended_tick, Some(30));
        assert_eq!(list[0].result.as_deref(), Some("all green"));
    }

    #[test]
    fn failed_subagent_and_unknown_actors() {
        let mut list = Vec::new();
        observe(&mut list, &task("A", "explorer: map"), 1);
        observe(&mut list, &task("A", "explorer: map"), 2);
        assert_eq!(list.len(), 1, "a repeated start is ignored");
        observe(
            &mut list,
            &AgentEvent::Text {
                text: "x".into(),
                actor: sub("Z"),
            },
            3,
        );
        observe(
            &mut list,
            &AgentEvent::ToolResult {
                tool_use_id: "A".into(),
                is_error: true,
                preview: "".into(),
                actor: Actor::Lead,
            },
            4,
        );
        assert_eq!(list[0].status, SubagentStatus::Failed);
        assert!(list[0].marks.is_empty());
    }

    #[test]
    fn summary_without_a_type() {
        let mut list = Vec::new();
        observe(&mut list, &task("A", "Task"), 1);
        assert_eq!(
            (list[0].kind.as_str(), list[0].description.as_str()),
            ("Task", "Task")
        );
    }
}
