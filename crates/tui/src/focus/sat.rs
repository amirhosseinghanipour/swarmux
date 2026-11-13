//! The situation-awareness card: what the agent is doing, why, what comes
//! next, how sure swarmux is and what it is unsure about. Every line comes
//! from something swarmux saw; nothing is guessed by a model.

use swarmux_application::GateState;
use swarmux_domain::gate::GateOutcome;
use swarmux_domain::telemetry::{Todo, TodoStatus};
use swarmux_domain::{JobStatus, ReviewState, ReviewVerdict, SessionState};

use super::stream::{Row, RowKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
    pub fn word(self) -> &'static str {
        match self {
            Confidence::Low => "low",
            Confidence::Medium => "medium",
            Confidence::High => "high",
        }
    }

    /// Filled cells out of five.
    pub fn cells(self) -> usize {
        match self {
            Confidence::Low => 1,
            Confidence::Medium => 3,
            Confidence::High => 5,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sat {
    pub doing: String,
    pub because: String,
    pub next: String,
    pub confidence: Confidence,
    /// Why the confidence is not higher; empty when nothing is flagged.
    pub unsure: Vec<String>,
}

/// What the card is worked out from.
pub struct Signals<'a> {
    pub status: JobStatus,
    pub goal: &'a str,
    pub rows: &'a [Row],
    pub todos: &'a [Todo],
    pub gate: Option<&'a GateState>,
    pub review: Option<&'a ReviewState>,
    pub loop_stops: usize,
    pub state: Option<SessionState>,
    /// The tool call held for the user, as `tool subject`.
    pub held: Option<String>,
    pub error: Option<&'a str>,
    pub answer: Option<&'a str>,
}

fn first_line(s: &str) -> &str {
    s.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
}

/// Tool results at the end of the stream that went wrong, since the last
/// one that went right.
fn trailing_errors(rows: &[Row]) -> usize {
    rows.iter()
        .rev()
        .filter(|r| matches!(r.kind, RowKind::Result { .. }))
        .take_while(|r| !matches!(r.kind, RowKind::Result { ok: true }))
        .count()
}

pub fn card(s: &Signals) -> Sat {
    let (confidence, unsure) = confidence(s);
    Sat {
        doing: doing(s),
        because: because(s),
        next: next(s),
        confidence,
        unsure,
    }
}

fn doing(s: &Signals) -> String {
    if let Some(held) = &s.held {
        return format!("waiting for your answer on {held}");
    }
    match s.status {
        JobStatus::Queued => return "waiting in the queue".into(),
        JobStatus::Done => {
            let said = s.answer.map(first_line).filter(|l| !l.is_empty());
            return match said {
                Some(l) => format!("finished: {l}"),
                None => "finished".into(),
            };
        }
        JobStatus::Failed | JobStatus::Interrupted => {
            return match s.error {
                Some(e) => format!("failed: {}", first_line(e)),
                None => "failed".into(),
            };
        }
        JobStatus::Cancelled => return "cancelled".into(),
        JobStatus::Running => {}
    }
    if s.state == Some(SessionState::RequiresAction) {
        return "waiting for you in the session".into();
    }
    let last = s
        .rows
        .iter()
        .rev()
        .find(|r| matches!(r.kind, RowKind::Call { .. } | RowKind::Say));
    match last {
        Some(r) => match &r.kind {
            RowKind::Call { tool, subject } => {
                let who = if r.depth > 0 {
                    format!("{}: ", r.actor)
                } else {
                    String::new()
                };
                format!("{who}{tool} {subject}").trim().to_string()
            }
            _ => format!("saying: {}", r.text),
        },
        None => "starting".into(),
    }
}

fn because(s: &Signals) -> String {
    match s.todos.iter().find(|t| t.status == TodoStatus::InProgress) {
        Some(t) => {
            let done = s
                .todos
                .iter()
                .filter(|t| t.status == TodoStatus::Completed)
                .count();
            format!(
                "step {} of {}: {}",
                done + 1,
                s.todos.len(),
                first_line(&t.content)
            )
        }
        None => format!("you asked: {}", first_line(s.goal)),
    }
}

fn next(s: &Signals) -> String {
    if s.held.is_some() {
        return "continues once you answer".into();
    }
    if let Some(g) = s.gate {
        match g.outcome {
            GateOutcome::Block => {
                return format!("fix and stop again (gate {}/{})", g.blocks, g.max_blocks);
            }
            GateOutcome::Escalate => return "your call: the gate gave up".into(),
            GateOutcome::Pass => {}
        }
    }
    if let Some(t) = s.todos.iter().find(|t| t.status == TodoStatus::Pending) {
        return first_line(&t.content).to_string();
    }
    match (s.status, s.review) {
        (JobStatus::Done, Some(ReviewState::Requested | ReviewState::Running { .. })) => {
            "review of the diff".into()
        }
        (JobStatus::Done, _) => "nothing: ready for you".into(),
        (JobStatus::Running, _) => "no plan listed by the agent".into(),
        _ => "nothing queued".into(),
    }
}

/// How sure swarmux is that this job is on track, and why not more.
fn confidence(s: &Signals) -> (Confidence, Vec<String>) {
    let mut low = Vec::new();
    let mut mid = Vec::new();
    if matches!(s.status, JobStatus::Failed | JobStatus::Interrupted) {
        low.push("the job failed".to_string());
    }
    if let Some(g) = s.gate {
        match g.outcome {
            GateOutcome::Block => low.push(format!("gate blocked {}/{}", g.blocks, g.max_blocks)),
            GateOutcome::Escalate => low.push("gate gave up".into()),
            GateOutcome::Pass => {}
        }
        if g.protected_changed > 0 {
            low.push(format!("{} protected file(s) changed", g.protected_changed));
        }
        if g.tampering > 0 {
            low.push(format!("{} way(s) the tests were weakened", g.tampering));
        }
        if g.notes.substrate > 0 {
            low.push(format!(
                "{} file(s) that run the tests were created or changed",
                g.notes.substrate
            ));
        }
        if g.base_suite == Some(false) {
            low.push("the original tests fail against the new code".into());
        }
        if let Some(note) = &g.notes.hidden_not_run {
            mid.push(note.clone());
        }
        if g.notes.ran_in == Some(swarmux_domain::gate::RanIn::Worktree) {
            mid.push("the checks ran in the worktree, not a clean copy".into());
        }
        if g.notes.dependencies > 0 {
            mid.push(format!(
                "{} manifest(s) gained dependencies",
                g.notes.dependencies
            ));
        }
        if let Some(h) = g.hidden.filter(|h| !h.clean()) {
            low.push(format!("{} of {} hidden checks failed", h.failed(), h.ran));
        }
        if g.evidence_gaps > 0 {
            mid.push("tests claimed but not seen running".to_string());
        }
        if g.checks.iter().any(|(_, _, flaky)| *flaky) {
            mid.push("a check was flaky".into());
        }
        if let Some(p) = &g.problem {
            mid.push(format!("gate file: {}", first_line(p)));
        }
    } else if s.status == JobStatus::Done {
        mid.push("no gate verdict".into());
    }
    if s.loop_stops > 0 {
        low.push(format!("loop breaker stopped it {}x", s.loop_stops));
    }
    let errs = trailing_errors(s.rows);
    if errs >= 3 {
        low.push(format!("last {errs} tool calls failed"));
    } else if errs > 0 {
        mid.push(format!("last tool call failed ({errs})"));
    }
    match s.review {
        Some(ReviewState::Done { review, .. }) if review.verdict == ReviewVerdict::Concerns => {
            mid.push(format!("reviewer has {} concern(s)", review.concerns.len()));
        }
        Some(ReviewState::Failed { .. }) => mid.push("review gave no verdict".into()),
        Some(ReviewState::Requested | ReviewState::Running { .. }) => {
            mid.push("review pending".into());
        }
        _ => {}
    }
    if s.held.is_some() {
        mid.push("a tool call waits for you".into());
    }
    let level = if !low.is_empty() {
        Confidence::Low
    } else if !mid.is_empty() || s.status != JobStatus::Done {
        Confidence::Medium
    } else {
        Confidence::High
    };
    let mut why = low;
    why.extend(mid);
    if why.is_empty() && level == Confidence::Medium {
        why.push("no checks have run yet".into());
    }
    (level, why)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate(outcome: GateOutcome) -> GateState {
        GateState {
            outcome,
            blocks: 1,
            max_blocks: 3,
            checks: vec![("tests".into(), outcome == GateOutcome::Pass, false)],
            protected_changed: 0,
            evidence_gaps: 0,
            hidden: None,
            tampering: 0,
            checks_weakened: false,
            base_suite: None,
            digest: String::new(),
            problem: None,
            at: 0,
            notes: Default::default(),
        }
    }

    fn base<'a>() -> Signals<'a> {
        Signals {
            status: JobStatus::Running,
            goal: "add /health",
            rows: &[],
            todos: &[],
            gate: None,
            review: None,
            loop_stops: 0,
            state: None,
            held: None,
            error: None,
            answer: None,
        }
    }

    #[test]
    fn a_passed_gate_on_a_finished_job_is_high() {
        let g = gate(GateOutcome::Pass);
        let s = Signals {
            status: JobStatus::Done,
            gate: Some(&g),
            answer: Some("Added it.\nmore"),
            ..base()
        };
        let sat = card(&s);
        assert_eq!(sat.confidence, Confidence::High);
        assert!(sat.unsure.is_empty());
        assert_eq!(sat.doing, "finished: Added it.");
        assert_eq!(sat.next, "nothing: ready for you");
    }

    #[test]
    fn a_blocked_gate_is_low_and_says_why() {
        let g = gate(GateOutcome::Block);
        let s = Signals {
            gate: Some(&g),
            loop_stops: 1,
            ..base()
        };
        let sat = card(&s);
        assert_eq!(sat.confidence, Confidence::Low);
        assert_eq!(sat.unsure.len(), 2);
        assert!(sat.next.contains("gate 1/3"));
    }

    #[test]
    fn plain_running_job_is_medium_with_a_reason() {
        let sat = card(&base());
        assert_eq!(sat.confidence, Confidence::Medium);
        assert_eq!(sat.unsure, ["no checks have run yet"]);
        assert_eq!(sat.because, "you asked: add /health");
        assert_eq!(sat.doing, "starting");
    }

    #[test]
    fn todos_give_the_reason_and_the_next_step() {
        let todo = |content: &str, status| Todo {
            content: content.into(),
            status,
            active_form: None,
        };
        let todos = [
            todo("write handler", TodoStatus::Completed),
            todo("add tests", TodoStatus::InProgress),
            todo("run clippy", TodoStatus::Pending),
        ];
        let sat = card(&Signals {
            todos: &todos,
            ..base()
        });
        assert_eq!(sat.because, "step 2 of 3: add tests");
        assert_eq!(sat.next, "run clippy");
    }

    #[test]
    fn a_held_call_comes_first() {
        let sat = card(&Signals {
            held: Some("Bash rm -rf x".into()),
            ..base()
        });
        assert!(sat.doing.contains("Bash rm -rf x"));
        assert_eq!(sat.next, "continues once you answer");
    }
}
