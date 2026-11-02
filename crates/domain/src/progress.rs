//! Live progress of one job, built purely from agent events.
//!
//! Time is given as a `tick` (tenths of a second since the job started) so this
//! stays deterministic and testable.

use std::collections::{HashMap, VecDeque};

use crate::{Actor, AgentEvent, JobId, JobResult, JobStatus};

const LOG_CAP: usize = 500;
const TEXT_MAX: usize = 160;

/// One timeline row: when a single actor was active.
#[derive(Clone, Debug, PartialEq)]
pub struct Lane {
    pub actor: Actor,
    pub label: String,
    /// Ticks at which the actor emitted an event.
    pub marks: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct JobProgress {
    pub job_id: JobId,
    pub session_name: String,
    pub model: String,
    pub status: JobStatus,
    pub log: VecDeque<String>,
    pub lanes: Vec<Lane>,
    pub cost_usd: f64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// Lead context in use (input + cache read + cache creation of the last turn).
    pub context_used: u64,
    pub context_window: Option<u64>,
    pub subagents_spawned: u32,
    pub last_tick: u32,
    /// Subagent tool_use id → label (e.g. "explorer").
    labels: HashMap<String, String>,
}

impl JobProgress {
    pub fn new(job_id: JobId, session_name: &str, model: &str) -> JobProgress {
        JobProgress {
            job_id,
            session_name: session_name.to_string(),
            model: model.to_string(),
            status: JobStatus::Running,
            log: VecDeque::new(),
            lanes: vec![Lane {
                actor: Actor::Lead,
                label: "lead".into(),
                marks: Vec::new(),
            }],
            cost_usd: 0.0,
            tokens_in: 0,
            tokens_out: 0,
            context_used: 0,
            context_window: None,
            subagents_spawned: 0,
            last_tick: 0,
            labels: HashMap::new(),
        }
    }

    pub fn label_for(&self, actor: &Actor) -> String {
        match actor {
            Actor::Lead => "lead".into(),
            Actor::Subagent(id) => self.labels.get(id).cloned().unwrap_or_else(|| id.clone()),
        }
    }

    /// Applies an event. Returns the job's final result when the event ends the job.
    pub fn apply(&mut self, ev: &AgentEvent, tick: u32) -> Option<JobResult> {
        self.last_tick = self.last_tick.max(tick);
        if matches!(ev, AgentEvent::Ignored { .. }) {
            return None;
        }
        if let AgentEvent::ToolUse {
            id, summary, name, ..
        } = ev
            && ev.starts_subagent()
        {
            let label = summary.split(':').next().unwrap_or_default().trim();
            let label = if label.is_empty() { name } else { label };
            self.labels.insert(id.clone(), label.to_string());
        }
        self.mark(ev.actor(), tick);

        match ev {
            AgentEvent::Usage {
                input,
                cache_read,
                cache_creation,
                actor: Actor::Lead,
                ..
            } => {
                self.context_used = input + cache_read + cache_creation;
                None
            }
            AgentEvent::Usage { .. } => None,
            AgentEvent::Init { model, .. } => {
                self.model = model.clone();
                self.log(ev, tick);
                None
            }
            AgentEvent::Result {
                is_error,
                subtype,
                cost_usd,
                duration_ms,
                tokens_in,
                tokens_out,
                cache_read,
                context_window,
                subagents_spawned,
                text,
                ..
            } => {
                self.cost_usd = *cost_usd;
                self.tokens_in = *tokens_in;
                self.tokens_out = *tokens_out;
                self.context_window = *context_window;
                self.subagents_spawned = *subagents_spawned;
                self.status = if *is_error {
                    JobStatus::Failed
                } else {
                    JobStatus::Done
                };
                self.log(ev, tick);
                Some(JobResult {
                    tokens_in: *tokens_in,
                    tokens_out: *tokens_out,
                    cache_read: *cache_read,
                    cost_usd: *cost_usd,
                    duration_ms: *duration_ms,
                    outcome: subtype.clone(),
                    error: None,
                    text: text.clone(),
                })
            }
            _ => {
                self.log(ev, tick);
                None
            }
        }
    }

    /// Ends the job as failed (unless it already finished). Returns the result to save.
    pub fn fail(&mut self, error: &str, tick: u32) -> Option<JobResult> {
        self.end(JobStatus::Failed, "error", Some(error), tick)
    }

    /// Ends the job as cancelled (unless it already finished). Returns the result to save.
    pub fn cancel(&mut self, tick: u32) -> Option<JobResult> {
        self.end(JobStatus::Cancelled, "cancelled", None, tick)
    }

    /// Ends the job as cancelled by swarmux itself, with `reason` as its error.
    pub fn stop(&mut self, reason: &str, tick: u32) -> Option<JobResult> {
        self.end(JobStatus::Cancelled, "cancelled", Some(reason), tick)
    }

    fn end(
        &mut self,
        status: JobStatus,
        kind: &str,
        error: Option<&str>,
        tick: u32,
    ) -> Option<JobResult> {
        if self.status.is_finished() {
            return None;
        }
        self.last_tick = self.last_tick.max(tick);
        self.status = status;
        self.push_log(tick, "swarmux", kind, error.unwrap_or("by user"));
        Some(JobResult {
            tokens_in: self.tokens_in,
            tokens_out: self.tokens_out,
            cache_read: 0,
            cost_usd: self.cost_usd,
            duration_ms: u64::from(tick) * 100,
            outcome: status.as_str().into(),
            error: error.map(str::to_string),
            text: String::new(),
        })
    }

    /// Adds a stored event back (kind, actor key and text as saved), for
    /// viewing a job from an earlier run.
    pub fn replay(&mut self, actor_key: &str, kind: &str, text: &str, tick: u32) {
        let actor = match actor_key {
            "lead" => Actor::Lead,
            id => Actor::Subagent(id.to_string()),
        };
        self.last_tick = self.last_tick.max(tick);
        self.mark(&actor, tick);
        let label = self.label_for(&actor);
        self.push_log(tick, &label, kind, text);
    }

    fn mark(&mut self, actor: &Actor, tick: u32) {
        let label = self.label_for(actor);
        match self.lanes.iter_mut().find(|l| &l.actor == actor) {
            Some(lane) => lane.marks.push(tick),
            None => self.lanes.push(Lane {
                actor: actor.clone(),
                label,
                marks: vec![tick],
            }),
        }
    }

    fn log(&mut self, ev: &AgentEvent, tick: u32) {
        let label = self.label_for(ev.actor());
        self.push_log(tick, &label, ev.kind(), &describe(ev));
    }

    fn push_log(&mut self, tick: u32, label: &str, kind: &str, text: &str) {
        let secs = tick / 10;
        self.log.push_back(format!(
            "{:02}:{:02} {label:<10} {kind:<11} {text}",
            secs / 60,
            secs % 60
        ));
        while self.log.len() > LOG_CAP {
            self.log.pop_front();
        }
    }
}

/// Short human text for an event (log lines and stored payloads).
pub fn describe(ev: &AgentEvent) -> String {
    let s = match ev {
        AgentEvent::Init { model, cwd, .. } => format!("{model} in {cwd}"),
        AgentEvent::Text { text, .. } => text.lines().next().unwrap_or_default().to_string(),
        AgentEvent::ToolUse { name, summary, .. } if summary == name => name.clone(),
        AgentEvent::ToolUse { name, summary, .. } => format!("{name} {summary}"),
        AgentEvent::ToolResult {
            is_error, preview, ..
        } => {
            let first = preview.lines().next().unwrap_or_default();
            if *is_error {
                format!("ERROR {first}")
            } else {
                first.to_string()
            }
        }
        AgentEvent::Usage { input, output, .. } => format!("in {input} out {output}"),
        AgentEvent::Result {
            cost_usd,
            duration_ms,
            num_turns,
            ..
        } => format!(
            "${cost_usd:.4} · {:.1}s · {num_turns} turns",
            *duration_ms as f64 / 1000.0
        ),
        AgentEvent::Ignored { kind } | AgentEvent::Unknown { kind, .. } => kind.clone(),
        AgentEvent::Malformed { raw } => raw.clone(),
    };
    match s.char_indices().nth(TEXT_MAX) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress() -> JobProgress {
        JobProgress::new(JobId(1), "demo", "sonnet")
    }

    fn result(cost: f64, is_error: bool) -> AgentEvent {
        AgentEvent::Result {
            is_error,
            subtype: "success".into(),
            cost_usd: cost,
            duration_ms: 1500,
            num_turns: 1,
            tokens_in: 10,
            tokens_out: 20,
            cache_read: 3,
            context_window: Some(1_000_000),
            subagents_spawned: 0,
            text: "all good".into(),
        }
    }

    #[test]
    fn result_finishes_job_and_returns_numbers() {
        let mut p = progress();
        let r = p.apply(&result(0.5, false), 15).unwrap();
        assert_eq!(p.status, JobStatus::Done);
        assert_eq!(p.cost_usd, 0.5);
        assert_eq!(r.cost_usd, 0.5);
        assert_eq!(r.cache_read, 3);
        assert_eq!(r.outcome, "success");
        assert_eq!(r.text, "all good");
        assert!(p.log.back().unwrap().starts_with("00:01 lead"));
    }

    #[test]
    fn error_result_marks_failed() {
        let mut p = progress();
        p.apply(&result(0.1, true), 1);
        assert_eq!(p.status, JobStatus::Failed);
    }

    #[test]
    fn fail_after_finish_is_ignored() {
        let mut p = progress();
        p.apply(&result(0.1, false), 1);
        assert_eq!(p.fail("late exit", 2), None);
        assert_eq!(p.status, JobStatus::Done);
    }

    #[test]
    fn fail_records_error() {
        let mut p = progress();
        let r = p.fail("spawn failed: x", 30).unwrap();
        assert_eq!(p.status, JobStatus::Failed);
        assert_eq!(r.error.as_deref(), Some("spawn failed: x"));
        assert_eq!(r.duration_ms, 3000);
        assert!(p.log.back().unwrap().contains("spawn failed"));
    }

    #[test]
    fn cancel_ends_running_job_once() {
        let mut p = progress();
        let r = p.cancel(20).unwrap();
        assert_eq!(p.status, JobStatus::Cancelled);
        assert_eq!((r.outcome.as_str(), r.error), ("cancelled", None));
        assert!(p.log.back().unwrap().contains("cancelled"));
        assert_eq!(p.cancel(21), None);
        assert_eq!(p.fail("late", 22), None);
    }

    #[test]
    fn stop_records_the_reason() {
        let mut p = progress();
        let r = p
            .stop("loop guard: Bash failed the same way twice", 5)
            .unwrap();
        assert_eq!(p.status, JobStatus::Cancelled);
        assert_eq!(
            r.error.as_deref(),
            Some("loop guard: Bash failed the same way twice")
        );
        assert!(p.log.back().unwrap().contains("loop guard"));
        assert_eq!(p.stop("again", 6), None);
    }

    #[test]
    fn replay_rebuilds_log_and_lanes() {
        let mut p = progress();
        p.replay("lead", "text", "hello", 5);
        p.replay("toolu_A", "tool_use", "Read a.rs", 12);
        assert_eq!(p.lanes.len(), 2);
        assert_eq!(p.lanes[1].marks, [12]);
        assert_eq!(p.last_tick, 12);
        assert!(p.log[0].starts_with("00:00 lead       text        hello"));
        assert!(p.log[1].contains("toolu_A"));
    }

    #[test]
    fn subagent_gets_labelled_lane() {
        let mut p = progress();
        p.apply(
            &AgentEvent::ToolUse {
                id: "toolu_A".into(),
                name: "Task".into(),
                summary: "explorer: map code".into(),
                actor: Actor::Lead,
            },
            1,
        );
        p.apply(
            &AgentEvent::Text {
                text: "looking".into(),
                actor: Actor::Subagent("toolu_A".into()),
            },
            4,
        );
        let labels: Vec<_> = p.lanes.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(labels, ["lead", "explorer"]);
        assert_eq!(p.lanes[1].marks, [4]);
        assert!(p.log.back().unwrap().contains("explorer"));
    }

    #[test]
    fn lead_usage_sets_context() {
        let mut p = progress();
        p.apply(
            &AgentEvent::Usage {
                input: 2,
                output: 5,
                cache_read: 100,
                cache_creation: 10,
                actor: Actor::Lead,
            },
            1,
        );
        assert_eq!(p.context_used, 112);
        assert!(p.log.is_empty(), "usage is not logged");
    }

    #[test]
    fn log_is_capped() {
        let mut p = progress();
        for i in 0..600 {
            p.apply(
                &AgentEvent::Text {
                    text: format!("{i}"),
                    actor: Actor::Lead,
                },
                i,
            );
        }
        assert_eq!(p.log.len(), LOG_CAP);
    }

    #[test]
    fn describe_truncates_long_text() {
        let ev = AgentEvent::Text {
            text: "x".repeat(400),
            actor: Actor::Lead,
        };
        assert_eq!(describe(&ev).chars().count(), TEXT_MAX + 1);
    }
}
