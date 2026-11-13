//! Guard holds: a tool call the Guard held for the user or refused, with
//! the rule, the target, the risk, who decided, the countdown, and what
//! the agent itself wrote just before the call.
//!
//! The agent's words are read from the job's stored session stream (the
//! `events` the job service appends as the run goes: text, tool calls and
//! results). They are quoted, never summarised or guessed at. The stream
//! keeps the first line of each text the agent wrote, so that is what is
//! quoted.

use swarmux_domain::{AttentionItem, HeldCall, JobId, Millis, Priority, is_destructive};

use super::outcome::outcome_of;
use super::service::Sources;
use super::view::{
    AgentWords, DecisionSource, HoldState, HoldView, PriorityKind, RiskView, StepKind, StepView,
};
use crate::ports::StoredEvent;
use crate::{DecisionRecord, Outcome, Source};

/// Most stored events read to find a call's words and steps. Usage
/// events sit between the text and the tool calls, so the window is
/// wider than what is shown.
pub const STREAM_WINDOW: usize = 60;
/// Steps of the job shown with a hold.
const STEPS_SHOWN: usize = 6;
/// Longest quote of the agent's words, in characters.
const WORDS_MAX: usize = 400;
/// Refusals listed: the newest.
const DENIED_SHOWN: usize = 10;
/// What the quote is, for a client that labels it.
const LABEL: &str = "the agent's own words, quoted from its session stream";

/// The holds of the moment: held calls most urgent first, then the
/// newest refusals.
pub(super) fn build(src: &Sources<'_>) -> Vec<HoldView> {
    let mut held: Vec<(Priority, Millis, Millis, HoldView)> = src
        .approvals
        .iter()
        .map(|a| {
            let req = &a.request;
            let deadline = src.now + Millis::try_from(a.secs_left).unwrap_or(0) * 1_000;
            let call = HeldCall {
                request_id: req.request_id,
                job: req.job_id,
                session: None,
                tool: &req.tool_name,
                subject: &req.subject,
                cwd: &req.cwd,
            };
            let fallback;
            let item = match src
                .attention
                .iter()
                .find(|i| i.request == Some(req.request_id))
            {
                Some(i) => i,
                None => {
                    fallback = AttentionItem::approval(&call, src.now, deadline);
                    &fallback
                }
            };
            let record = src.decisions.iter().find(|d| {
                d.outcome == Outcome::Ask
                    && d.job_id == req.job_id
                    && d.tool == req.tool_name
                    && d.subject == req.subject
            });
            let at = record.map_or(item.since, |d| d.ts);
            let (agent_words, steps) = stream(src, req.job_id, at);
            let view = HoldView {
                id: format!("hold:{}", req.request_id),
                state: HoldState::Held,
                request: Some(req.request_id),
                rule: record.and_then(|d| d.rule.clone()),
                knowledge_rule: a.rule.clone(),
                tool: req.tool_name.clone(),
                target: req.subject.clone(),
                cwd: Some(req.cwd.clone()).filter(|c| !c.is_empty()),
                risk: item.risk.map(|r| RiskView {
                    tier: r.tier.into(),
                    score: r.score,
                    secret: r.secret,
                }),
                priority: Some(PriorityKind::from(item.priority)),
                destructive: item.is_destructive_ask(),
                decided_by: record.map_or(Source::Guard, |d| d.source).into(),
                confidence: record.and_then(|d| d.confidence),
                at,
                deadline_at: Some(deadline),
                job: req.job_id.map(|j| j.0),
                outcome: req.job_id.map(|j| outcome_of(src, j)),
                session: session_of(src, req.job_id),
                agent_words,
                steps,
            };
            (item.priority, deadline, at, view)
        })
        .collect();
    held.sort_by(|a, b| (a.0, a.1, a.2, &a.3.id).cmp(&(b.0, b.1, b.2, &b.3.id)));
    let mut out: Vec<HoldView> = held.into_iter().map(|h| h.3).collect();

    let mut seen: Vec<String> = Vec::new();
    for d in src
        .decisions
        .iter()
        .filter(|d| d.outcome == Outcome::Deny)
        .filter(|d| d.job_id.is_some())
        .take(DENIED_SHOWN)
    {
        out.push(denied(src, d, &mut seen));
    }
    out
}

fn denied(src: &Sources<'_>, d: &DecisionRecord, seen: &mut Vec<String>) -> HoldView {
    let job = d.job_id;
    let mut id = format!("denied:{}:{}", job.map_or(0, |j| j.0), d.ts);
    let mut n = 1;
    while seen.contains(&id) {
        n += 1;
        id = format!("denied:{}:{}-{n}", job.map_or(0, |j| j.0), d.ts);
    }
    seen.push(id.clone());
    let (agent_words, steps) = stream(src, job, d.ts);
    HoldView {
        id,
        state: HoldState::Denied,
        request: None,
        rule: d.rule.clone(),
        knowledge_rule: None,
        tool: d.tool.clone(),
        target: d.subject.clone(),
        cwd: None,
        risk: None,
        priority: None,
        destructive: is_destructive(&d.tool, &d.subject, ""),
        decided_by: DecisionSource::from(d.source),
        confidence: d.confidence,
        at: d.ts,
        deadline_at: None,
        job: job.map(|j| j.0),
        outcome: job.map(|j| outcome_of(src, j)),
        session: session_of(src, job),
        agent_words,
        steps,
    }
}

fn session_of(src: &Sources<'_>, job: Option<JobId>) -> Option<String> {
    let job = job?;
    src.facts
        .iter()
        .find(|f| f.job_id == job)
        .map(|f| f.session_name.clone())
}

/// What the agent wrote just before `at`, and its last steps, from the
/// job's stored stream.
fn stream(
    src: &Sources<'_>,
    job: Option<JobId>,
    at: Millis,
) -> (Option<AgentWords>, Vec<StepView>) {
    let Some(job) = job else {
        return (None, Vec::new());
    };
    let events = src
        .stream
        .recent_events(job, at, STREAM_WINDOW)
        .unwrap_or_default();
    let mut steps: Vec<StepView> = events.iter().filter_map(step_of).collect();
    let words = steps
        .iter()
        .rposition(|s| s.kind == StepKind::Text && s.actor == "lead")
        .map(|i| {
            let (text, truncated) = cut(&steps[i].text, WORDS_MAX);
            AgentWords {
                label: LABEL.to_string(),
                text,
                at: steps[i].at,
                truncated,
                steps_since: u32::try_from(steps.len() - i - 1).unwrap_or(u32::MAX),
            }
        });
    let keep = steps.len().saturating_sub(STEPS_SHOWN);
    (words, steps.split_off(keep))
}

/// A stored event as a step, when it is one the action stream shows.
fn step_of(e: &StoredEvent) -> Option<StepView> {
    let kind = match e.kind.as_str() {
        "text" => StepKind::Text,
        "tool_use" => StepKind::ToolCall,
        "tool_result" => StepKind::ToolResult,
        _ => return None,
    };
    let (text, ok) = match kind {
        StepKind::ToolResult => match e.payload.strip_prefix("ERROR ") {
            Some(rest) => (rest.to_string(), Some(false)),
            None => (e.payload.clone(), Some(true)),
        },
        _ => (e.payload.clone(), None),
    };
    Some(StepView {
        at: e.ts,
        kind,
        actor: e.actor.clone(),
        text,
        ok,
    })
}

/// `text` cut to `max` characters, with `…` when it was; whether it was.
pub(super) fn cut(text: &str, max: usize) -> (String, bool) {
    let text = text.trim();
    match text.char_indices().nth(max) {
        Some((i, _)) => (format!("{}…", text[..i].trim_end()), true),
        None => (text.to_string(), false),
    }
}
