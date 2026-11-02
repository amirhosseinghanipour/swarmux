//! Stops agents that keep making the same failing tool call.

use std::collections::HashMap;

use crate::{Actor, AgentEvent};

const SIGNATURE_MAX: usize = 100;

/// The loop breaker fired.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopTrip {
    pub tool: String,
    pub summary: String,
    /// The repeated error's signature.
    pub error: String,
    pub repeats: u32,
}

impl LoopTrip {
    /// One line for the job error and the guard log.
    pub fn reason(&self) -> String {
        let times = match self.repeats {
            2 => "twice".to_string(),
            n => format!("{n} times"),
        };
        format!("loop guard: {} failed the same way {times}", self.tool)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Streak {
    key: (String, String, String),
    count: u32,
}

/// Watches one job's tool calls and results, per agent.
#[derive(Clone, Debug, Default)]
pub struct LoopBreaker {
    repeats: u32,
    /// tool_use id → (tool, summary).
    calls: HashMap<String, (String, String)>,
    /// Current run of identical failures per agent.
    streaks: HashMap<Actor, Streak>,
}

impl LoopBreaker {
    /// Fires after `repeats` identical failures in a row; 0 turns it off.
    pub fn new(repeats: u32) -> LoopBreaker {
        LoopBreaker {
            repeats,
            ..LoopBreaker::default()
        }
    }

    /// Feeds one event. Returns a trip when the same call (tool and summary)
    /// has failed with the same error `repeats` times in a row for one agent.
    pub fn observe(&mut self, ev: &AgentEvent) -> Option<LoopTrip> {
        if self.repeats == 0 {
            return None;
        }
        match ev {
            AgentEvent::ToolUse {
                id, name, summary, ..
            } => {
                self.calls
                    .insert(id.clone(), (name.clone(), summary.clone()));
                None
            }
            AgentEvent::ToolResult {
                tool_use_id,
                is_error,
                preview,
                actor,
            } => {
                let call = self.calls.remove(tool_use_id);
                let (Some((tool, summary)), true) = (call, *is_error) else {
                    self.streaks.remove(actor);
                    return None;
                };
                let key = (tool, summary, error_signature(preview));
                let streak = self.streaks.entry(actor.clone()).or_insert(Streak {
                    key: key.clone(),
                    count: 0,
                });
                if streak.key == key {
                    streak.count += 1;
                } else {
                    *streak = Streak { key, count: 1 };
                }
                (streak.count >= self.repeats).then(|| {
                    let (tool, summary, error) = streak.key.clone();
                    LoopTrip {
                        tool,
                        summary,
                        error,
                        repeats: streak.count,
                    }
                })
            }
            _ => None,
        }
    }
}

/// An error message reduced to what stays the same between attempts: the
/// first non-empty line, lower case, digits as `#`, spaces collapsed.
pub fn error_signature(preview: &str) -> String {
    let line = preview
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    let reduced: String = line
        .chars()
        .map(|c| {
            if c.is_ascii_digit() {
                '#'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect();
    reduced
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(SIGNATURE_MAX)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, cmd: &str) -> AgentEvent {
        AgentEvent::ToolUse {
            id: id.into(),
            name: "Bash".into(),
            summary: cmd.into(),
            actor: Actor::Lead,
        }
    }

    fn result(id: &str, is_error: bool, preview: &str) -> AgentEvent {
        AgentEvent::ToolResult {
            tool_use_id: id.into(),
            is_error,
            preview: preview.into(),
            actor: Actor::Lead,
        }
    }

    fn fail_twice(b: &mut LoopBreaker, cmd: &str, err1: &str, err2: &str) -> Option<LoopTrip> {
        b.observe(&call("1", cmd));
        assert_eq!(b.observe(&result("1", true, err1)), None);
        b.observe(&call("2", cmd));
        b.observe(&result("2", true, err2))
    }

    #[test]
    fn same_failure_twice_trips() {
        let mut b = LoopBreaker::new(2);
        let trip = fail_twice(&mut b, "cargo test", "error at 10:01", "error at 10:02").unwrap();
        assert_eq!(trip.tool, "Bash");
        assert_eq!(trip.summary, "cargo test");
        assert_eq!(trip.reason(), "loop guard: Bash failed the same way twice");
    }

    #[test]
    fn different_errors_or_calls_do_not_trip() {
        let mut b = LoopBreaker::new(2);
        assert_eq!(
            fail_twice(&mut b, "x", "not found", "permission denied"),
            None
        );
        let mut b = LoopBreaker::new(2);
        b.observe(&call("1", "a"));
        b.observe(&result("1", true, "boom"));
        b.observe(&call("2", "b"));
        assert_eq!(b.observe(&result("2", true, "boom")), None);
    }

    #[test]
    fn success_in_between_resets() {
        let mut b = LoopBreaker::new(2);
        b.observe(&call("1", "a"));
        b.observe(&result("1", true, "boom"));
        b.observe(&call("2", "ls"));
        b.observe(&result("2", false, "ok"));
        b.observe(&call("3", "a"));
        assert_eq!(b.observe(&result("3", true, "boom")), None);
    }

    #[test]
    fn agents_are_tracked_separately() {
        let mut b = LoopBreaker::new(2);
        let sub = Actor::Subagent("t".into());
        b.observe(&call("1", "a"));
        b.observe(&result("1", true, "boom"));
        b.observe(&AgentEvent::ToolUse {
            id: "s".into(),
            name: "Read".into(),
            summary: "x".into(),
            actor: sub.clone(),
        });
        b.observe(&AgentEvent::ToolResult {
            tool_use_id: "s".into(),
            is_error: false,
            preview: "fine".into(),
            actor: sub,
        });
        b.observe(&call("2", "a"));
        assert!(b.observe(&result("2", true, "boom")).is_some());
    }

    #[test]
    fn configurable_and_off() {
        let mut b = LoopBreaker::new(3);
        assert_eq!(fail_twice(&mut b, "a", "e", "e"), None);
        b.observe(&call("3", "a"));
        let trip = b.observe(&result("3", true, "e")).unwrap();
        assert_eq!(
            trip.reason(),
            "loop guard: Bash failed the same way 3 times"
        );
        let mut off = LoopBreaker::new(0);
        assert_eq!(fail_twice(&mut off, "a", "e", "e"), None);
    }

    #[test]
    fn signature_ignores_numbers_case_and_later_lines() {
        assert_eq!(
            error_signature("\n  Error: Exit code 101 \nmore"),
            "error: exit code ###"
        );
        assert_eq!(error_signature(""), "");
    }
}
