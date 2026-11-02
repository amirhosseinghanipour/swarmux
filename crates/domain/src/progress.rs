//! Live progress of one job, built purely from agent events.
//!
//! Time is given as a `tick` (tenths of a second since the job started) so this
//! stays deterministic and testable.

use std::collections::{HashMap, VecDeque};

use crate::subagent::{self, Subagent, SubagentStatus};
use crate::{
    Actor, AgentEvent, JobId, JobResult, JobStatus, Phase, PhaseInput, ReviewState, TokenTally,
    phase_of,
};

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
    /// Subagents in the order they started.
    pub subagents: Vec<Subagent>,
    /// The lead's own usage.
    pub lead_tokens: TokenTally,
    /// When guard, routing and JEV decisions were made for this job.
    pub decision_marks: Vec<u32>,
    /// Why the loop breaker stopped an agent, oldest first.
    pub loop_stops: Vec<String>,
    /// `None` when no review was asked for.
    pub review: Option<ReviewState>,
    /// When each phase (in [`Phase::ALL`] order) was first reached.
    pub phase_ticks: [Option<u32>; 7],
    /// The command line the job runs with, when known.
    pub command: Option<String>,
    /// Name of the main actor in the log and timeline ("lead", or "reviewer").
    pub lead_label: String,
    /// Added to every logged time, so a review's log lines up with its job's.
    pub log_offset: u32,
    /// Cache-read tokens of the whole job, once Claude Code reported them.
    final_cache_read: Option<u64>,
    /// Subagent tool_use id → label (e.g. "explorer").
    labels: HashMap<String, String>,
    /// Subagent calls seen while replaying, not yet matched to an actor.
    replay_starts: VecDeque<(String, u32)>,
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
            subagents: Vec::new(),
            lead_tokens: TokenTally::default(),
            decision_marks: Vec::new(),
            loop_stops: Vec::new(),
            review: None,
            phase_ticks: [Some(0), None, None, None, None, None, None],
            command: None,
            lead_label: "lead".into(),
            log_offset: 0,
            final_cache_read: None,
            labels: HashMap::new(),
            replay_starts: VecDeque::new(),
        }
    }

    /// The step the job is at now.
    pub fn phase(&self) -> Phase {
        phase_of(PhaseInput {
            status: self.status,
            decisions: self.decision_marks.len() as u32,
            subagents_started: self.subagents.len(),
            subagents_running: self.subagents_running(),
            review: self.review.as_ref(),
        })
    }

    pub fn subagents_running(&self) -> usize {
        self.subagents.iter().filter(|s| s.is_running()).count()
    }

    /// Cache-read tokens so far: the final figure once known, else the sum
    /// of the usage reports.
    pub fn cache_read(&self) -> u64 {
        self.final_cache_read.unwrap_or_else(|| {
            self.lead_tokens.cache_read
                + self
                    .subagents
                    .iter()
                    .map(|s| s.tokens.cache_read)
                    .sum::<u64>()
        })
    }

    /// Makes this the progress of a review that started `offset` ticks into
    /// the reviewed job.
    pub fn as_reviewer(&mut self, offset: u32) {
        self.lead_label = "reviewer".into();
        self.log_offset = offset;
        if let Some(lane) = self.lanes.first_mut() {
            lane.label = "reviewer".into();
        }
    }

    /// Notes a guard, routing or JEV decision made for this job.
    pub fn record_decision(&mut self, tick: u32) {
        self.decision_marks.push(tick);
        self.refresh_phase(tick);
    }

    /// Notes that the loop breaker stopped an agent, and why.
    pub fn record_loop_stop(&mut self, reason: &str) {
        self.loop_stops.push(reason.to_string());
    }

    pub fn set_review(&mut self, review: Option<ReviewState>, tick: u32) {
        self.review = review;
        self.refresh_phase(tick);
    }

    /// Sets the model a subagent runs on (from its definition).
    pub fn set_subagent_model(&mut self, id: &str, model: Option<String>) {
        if let Some(s) = self.subagents.iter_mut().find(|s| s.id == id) {
            s.model = model;
        }
    }

    /// Records the first time the current phase was reached.
    fn refresh_phase(&mut self, tick: u32) {
        let slot = &mut self.phase_ticks[self.phase().index()];
        if slot.is_none() {
            *slot = Some(tick);
        }
    }

    /// Ends subagents still marked running when the job itself ends.
    fn close_subagents(&mut self, tick: u32) {
        let status = if self.status == JobStatus::Done {
            SubagentStatus::Done
        } else {
            SubagentStatus::Failed
        };
        for s in self.subagents.iter_mut().filter(|s| s.is_running()) {
            s.status = status;
            let last = s.marks.last().copied().unwrap_or(tick);
            s.ended_tick = Some(last.max(s.started_tick));
        }
    }

    pub fn label_for(&self, actor: &Actor) -> String {
        match actor {
            Actor::Lead => self.lead_label.clone(),
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
        subagent::observe(&mut self.subagents, ev, tick);
        let finished = self.apply_inner(ev, tick);
        if finished.is_some() {
            self.close_subagents(tick);
        }
        self.refresh_phase(tick);
        finished
    }

    fn apply_inner(&mut self, ev: &AgentEvent, tick: u32) -> Option<JobResult> {
        match ev {
            AgentEvent::Usage {
                input,
                output,
                cache_read,
                cache_creation,
                actor: Actor::Lead,
            } => {
                self.context_used = input + cache_read + cache_creation;
                self.lead_tokens
                    .add(*input, *output, *cache_read, *cache_creation, tick);
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
                self.final_cache_read = Some(*cache_read);
                // Claude Code reports again after background subagents end;
                // that updates the numbers but doesn't end the job twice.
                if !self.status.is_finished() {
                    self.status = if *is_error {
                        JobStatus::Failed
                    } else {
                        JobStatus::Done
                    };
                }
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
        self.close_subagents(tick);
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
        if actor == Actor::Lead
            && kind == "tool_use"
            && let Some(summary) = text
                .strip_prefix("Task ")
                .or_else(|| text.strip_prefix("Agent "))
        {
            self.replay_starts.push_back((summary.to_string(), tick));
        }
        if let Actor::Subagent(id) = &actor {
            self.replay_subagent(id, tick);
        }
        if kind == "usage"
            && let Some([input, output, read, creation]) = parse_usage(text)
        {
            let ev = AgentEvent::Usage {
                input,
                output,
                cache_read: read,
                cache_creation: creation,
                actor: actor.clone(),
            };
            self.mark(&actor, tick);
            subagent::observe(&mut self.subagents, &ev, tick);
            if actor == Actor::Lead {
                self.context_used = input + read + creation;
                self.lead_tokens.add(input, output, read, creation, tick);
            }
            return;
        }
        if let Actor::Subagent(id) = &actor
            && let Some(s) = self.subagents.iter_mut().find(|s| &s.id == id)
        {
            s.marks.push(tick);
        }
        self.mark(&actor, tick);
        let label = self.label_for(&actor);
        self.push_log(tick, &label, kind, text);
        self.refresh_phase(tick);
    }

    /// The first stored event of a subagent: match it to the oldest Task
    /// call not matched yet (stored calls carry no tool_use id).
    fn replay_subagent(&mut self, id: &str, tick: u32) {
        if self.subagents.iter().any(|s| s.id == id) {
            return;
        }
        let (summary, start) = self
            .replay_starts
            .pop_front()
            .unwrap_or_else(|| ("Task".to_string(), tick));
        let ev = AgentEvent::ToolUse {
            id: id.to_string(),
            name: "Task".into(),
            summary,
            actor: Actor::Lead,
        };
        subagent::observe(&mut self.subagents, &ev, start);
        if let Some(kind) = self.subagents.last().map(|s| s.kind.clone()) {
            self.labels.insert(id.to_string(), kind);
        }
    }

    /// Ends a replayed job: its status, and its subagents with it.
    pub fn finish_replay(&mut self, status: JobStatus) {
        self.status = status;
        self.close_subagents(self.last_tick);
        self.refresh_phase(self.last_tick);
    }

    /// Adds a line from swarmux itself, such as why a prompt came here.
    pub fn note(&mut self, kind: &str, text: &str) {
        self.push_log(self.last_tick, "swarmux", kind, text);
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
        let secs = (tick + self.log_offset) / 10;
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
        AgentEvent::Usage {
            input,
            output,
            cache_read,
            cache_creation,
            ..
        } => format!("in {input} out {output} cache {cache_read}/{cache_creation}"),
        AgentEvent::Result {
            cost_usd,
            duration_ms,
            num_turns,
            ..
        } => format!(
            "${cost_usd:.4} · {:.1}s · {num_turns} turns",
            *duration_ms as f64 / 1000.0
        ),
        AgentEvent::RateLimit(l) => {
            let used = l
                .utilization
                .map_or_else(|| "?".to_string(), |u| format!("{:.0}%", u * 100.0));
            format!("{} {used} ({})", l.window, l.status)
        }
        AgentEvent::Environment(env) => env.summary(),
        AgentEvent::Todos { todos, .. } => {
            let (done, doing, open) = crate::telemetry::todo_counts(todos);
            format!(
                "{} todos: {done} done, {doing} in progress, {open} open",
                todos.len()
            )
        }
        AgentEvent::Ignored { kind } | AgentEvent::Unknown { kind, .. } => kind.clone(),
        AgentEvent::Malformed { raw } => raw.clone(),
    };
    match s.char_indices().nth(TEXT_MAX) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s,
    }
}

/// Reads a stored usage line (`in 2 out 5 cache 100/10`; older rows have no
/// cache part).
fn parse_usage(text: &str) -> Option<[u64; 4]> {
    let w: Vec<&str> = text.split_whitespace().collect();
    let num = |s: &str| s.parse::<u64>().ok();
    match w.as_slice() {
        ["in", i, "out", o] => Some([num(i)?, num(o)?, 0, 0]),
        ["in", i, "out", o, "cache", c] => {
            let (r, cc) = c.split_once('/')?;
            Some([num(i)?, num(o)?, num(r)?, num(cc)?])
        }
        _ => None,
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
    fn a_later_result_updates_numbers_but_keeps_the_status() {
        let mut p = progress();
        p.apply(&result(0.1, false), 1);
        let again = p.apply(&result(0.3, true), 9).unwrap();
        assert_eq!(
            (p.status, p.cost_usd, again.cost_usd),
            (JobStatus::Done, 0.3, 0.3)
        );
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
        assert!(p.log[1].contains("Task"), "{}", p.log[1]);
        assert_eq!(p.subagents.len(), 1);
    }

    #[test]
    fn replay_matches_task_calls_and_reads_usage() {
        let mut p = progress();
        p.replay("lead", "tool_use", "Task worker: run tests", 2);
        p.replay("lead", "tool_use", "Agent explorer: map", 3);
        p.replay("lead", "usage", "in 2 out 10 cache 100/5", 4);
        p.replay("toolu_B", "text", "mapping", 6);
        p.replay("toolu_A", "usage", "in 1 out 30", 9);
        p.finish_replay(JobStatus::Done);
        let kinds: Vec<&str> = p.subagents.iter().map(|s| s.kind.as_str()).collect();
        assert_eq!(kinds, ["worker", "explorer"]);
        assert_eq!(p.subagents[0].started_tick, 2);
        assert_eq!(p.subagents[1].tokens.output, 30);
        assert_eq!(p.lead_tokens.cache_read, 100);
        assert_eq!(p.context_used, 107);
        assert!(p.subagents.iter().all(|s| s.status == SubagentStatus::Done));
        assert_eq!(p.phase(), Phase::Ship);
        assert_eq!(p.lanes[1].label, "worker");
        assert_eq!(parse_usage("in x out 1"), None);
    }

    fn task(id: &str, kind: &str) -> AgentEvent {
        AgentEvent::ToolUse {
            id: id.into(),
            name: "Task".into(),
            summary: format!("{kind}: work"),
            actor: Actor::Lead,
        }
    }

    fn finish(id: &str) -> AgentEvent {
        AgentEvent::ToolResult {
            tool_use_id: id.into(),
            is_error: false,
            preview: "ok".into(),
            actor: Actor::Lead,
        }
    }

    #[test]
    fn phases_are_reached_in_order_and_remembered() {
        let mut p = progress();
        assert_eq!(p.phase(), Phase::Plan);
        p.record_decision(3);
        assert_eq!(p.phase(), Phase::Fork);
        p.apply(&task("A", "worker"), 5);
        assert_eq!(p.phase(), Phase::Delegate);
        p.apply(&task("B", "explorer"), 6);
        assert_eq!((p.phase(), p.subagents_running()), (Phase::Parallel, 2));
        p.apply(&finish("A"), 9);
        p.apply(&finish("B"), 11);
        assert_eq!(p.phase(), Phase::Merge);
        p.set_review(Some(ReviewState::Requested), 11);
        p.apply(&result(0.2, false), 14);
        assert_eq!(p.phase(), Phase::Audit);
        let ticks: Vec<Option<u32>> = p.phase_ticks.to_vec();
        assert_eq!(
            ticks,
            [Some(0), Some(3), Some(5), Some(6), Some(11), Some(14), None]
        );
        p.set_review(None, 20);
        assert_eq!((p.phase(), p.phase_ticks[6]), (Phase::Ship, Some(20)));
    }

    #[test]
    fn lead_and_subagent_tokens_and_cache() {
        let mut p = progress();
        p.apply(&task("A", "worker"), 1);
        let usage = |actor: Actor, out: u64| AgentEvent::Usage {
            input: 1,
            output: out,
            cache_read: 40,
            cache_creation: 0,
            actor,
        };
        p.apply(&usage(Actor::Lead, 5), 2);
        p.apply(&usage(Actor::Lead, 5), 2);
        p.apply(&usage(Actor::Subagent("A".into()), 9), 3);
        assert_eq!((p.lead_tokens.turns, p.lead_tokens.seen), (1, 46));
        assert_eq!(p.subagents[0].tokens.output, 9);
        assert_eq!(p.cache_read(), 80);
        p.apply(&result(0.1, false), 4);
        assert_eq!(p.cache_read(), 3, "the final figure wins");
        assert_eq!(p.subagents[0].status, SubagentStatus::Done);
    }

    #[test]
    fn cancel_fails_running_subagents_and_keeps_loop_stops() {
        let mut p = progress();
        p.apply(&task("A", "worker"), 1);
        p.record_loop_stop("loop guard: Bash failed the same way twice");
        p.cancel(4);
        assert_eq!(p.subagents[0].status, SubagentStatus::Failed);
        assert_eq!(p.subagents[0].ended_tick, Some(4));
        assert_eq!(p.loop_stops.len(), 1);
        p.set_subagent_model("A", Some("haiku".into()));
        assert_eq!(p.subagents[0].model.as_deref(), Some("haiku"));
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
    fn reviewer_logs_with_offset() {
        let mut p = progress();
        p.as_reviewer(650);
        p.apply(
            &AgentEvent::Text {
                text: "checking".into(),
                actor: Actor::Lead,
            },
            20,
        );
        assert!(p.log[0].starts_with("01:07 reviewer"), "{}", p.log[0]);
        assert_eq!(p.lanes[0].label, "reviewer");
    }

    #[test]
    fn notes_are_logged_as_swarmux() {
        let mut p = progress();
        p.note("route", "routed → web");
        assert_eq!(p.log[0], "00:00 swarmux    route       routed → web");
        assert_eq!(p.lanes.len(), 1, "no lane for swarmux");
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
