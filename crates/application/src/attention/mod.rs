//! The attention inbox and the change log: what needs the user, in order,
//! and what changed that doesn't. Built from what the other services
//! already know; nothing is stored, so acknowledgements last for the run.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use swarmux_domain::alarm::{Flood, Raised, RiskTier, Tally, detect_flood, novelty, signature_of};
use swarmux_domain::{
    AttentionId, AttentionItem, CallContext, HeldCall, JobId, JobStatus, Millis, Priority,
    ReviewState, ReviewVerdict, STALL_AFTER_MS, SessionId, SessionState, rank,
};

use crate::{DecisionRecord, GuardService, JobFact, JobService, Outcome, PendingApproval, Source};

mod service;
pub mod store;

#[cfg(test)]
mod planted;

pub use service::{HeldBack, Suggestion, alarm_report};
pub use store::{AlarmEvent, AlarmKind, AlarmStore, ShelveFor, ShelvedItem, TallyRow};

/// What kind of change a log row reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Started,
    Finished,
    Failed,
    Reviewed,
    Denied,
    Asked,
    Stopped,
    Routed,
    /// The done-gate judged a stop.
    Gate,
    /// The watchdog saw trouble or took a recovery step.
    Watchdog,
}

/// One row of the change log: informational, never an inbox item.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub at: Millis,
    pub job: Option<JobId>,
    /// The session's name, `"?"` when the job isn't known.
    pub session: String,
    pub kind: ChangeKind,
    pub text: String,
}

/// Collects, ranks and acknowledges attention items, and keeps the inbox's
/// alarm discipline: shelving, focus mode, batching, floods, rule offers and
/// the figures that show whether the inbox is under control.
#[derive(Default)]
pub struct AttentionService {
    acknowledged: HashSet<AttentionId>,
    /// When each item was first seen, for those whose age isn't known.
    first_seen: HashMap<AttentionId, Millis>,
    store: Option<Rc<dyn AlarmStore>>,
    shelved: Vec<ShelvedItem>,
    /// Focus mode, since when.
    focus: Option<Millis>,
    /// Held-back items a breakpoint let through.
    released: HashSet<AttentionId>,
    /// Held back by focus mode at the last collection.
    held: Vec<(AttentionId, Priority)>,
    held_back: HeldBack,
    raised: Vec<Raised>,
    raised_seen: HashSet<AttentionId>,
    attended: HashSet<AttentionId>,
    ttds: Vec<Millis>,
    tallies: HashMap<(String, String), Tally>,
    /// The repository each open approval was asked in.
    scopes: HashMap<AttentionId, String>,
    /// Request ids that can be answered together, by the id of any member.
    members: HashMap<AttentionId, Vec<u64>>,
    flood: Option<Flood>,
    /// Everything open at the last collection, shown or not.
    open: Vec<AttentionItem>,
    error: Option<String>,
}

impl AttentionService {
    pub fn new() -> AttentionService {
        AttentionService::default()
    }

    /// Everything that needs the user now, most urgent first.
    pub fn collect(
        &mut self,
        guard: &GuardService,
        jobs: &JobService,
        now: Millis,
    ) -> Vec<AttentionItem> {
        self.collect_with(guard, jobs, Vec::new(), now)
    }

    /// [`collect`](Self::collect), plus `extra` items from other sources
    /// (the done-gate's escalations).
    pub fn collect_with(
        &mut self,
        guard: &GuardService,
        jobs: &JobService,
        extra: Vec<AttentionItem>,
        now: Millis,
    ) -> Vec<AttentionItem> {
        let worries: Vec<Worry> = guard.recent().filter_map(worry).collect();
        self.gather(&guard.approvals(), &jobs.facts(), extra, &worries, now)
    }

    /// [`collect`](Self::collect) from plain values.
    pub fn collect_from(
        &mut self,
        approvals: &[PendingApproval],
        facts: &[JobFact],
        now: Millis,
    ) -> Vec<AttentionItem> {
        self.gather(approvals, facts, Vec::new(), &[], now)
    }

    fn gather(
        &mut self,
        approvals: &[PendingApproval],
        facts: &[JobFact],
        extra: Vec<AttentionItem>,
        worries: &[Worry],
        now: Millis,
    ) -> Vec<AttentionItem> {
        let mut items = extra;
        self.scopes.clear();
        for a in approvals {
            let r = &a.request;
            let session = r
                .job_id
                .and_then(|j| facts.iter().find(|f| f.job_id == j))
                .map(|f| f.session_id);
            let call = HeldCall {
                request_id: r.request_id,
                job: r.job_id,
                session,
                tool: &r.tool_name,
                subject: &r.subject,
                cwd: &r.cwd,
            };
            let key = AttentionId::new(format!("approval:{}", r.request_id));
            let since = *self.first_seen.entry(key.clone()).or_insert(now);
            let deadline = now + Millis::try_from(a.secs_left).unwrap_or(0) * 1000;
            let seen = signature_of(&r.tool_name, &r.subject, RiskTier::Routine).key();
            let tally = self
                .tallies
                .get(&(r.cwd.clone(), seen))
                .copied()
                .unwrap_or_default();
            let ctx = CallContext {
                novelty: novelty(&tally),
                track_record: session.map_or(0.5, |s| track_record(s, facts)),
                taint: false,
                jev: worries
                    .iter()
                    .find(|w| w.tool == r.tool_name && w.subject == r.subject)
                    .map(|w| w.worry),
            };
            let mut item = AttentionItem::approval_scored(&call, &ctx, since, deadline);
            if let Some(rule) = &a.rule {
                // The owner asked to see these: always first.
                item.priority = swarmux_domain::Priority::P1;
                item.detail = rule.clone();
            }
            // A subagent's call says so: who is asking matters to the answer.
            let by = match (&r.agent_type, &r.agent_id) {
                (Some(kind), Some(id)) => Some(format!("subagent {kind} ({id})")),
                (Some(kind), None) => Some(format!("subagent {kind}")),
                (None, Some(id)) => Some(format!("subagent {id}")),
                (None, None) => None,
            };
            if let Some(by) = by {
                item.detail = format!("{by} · {}", item.detail);
            }
            if let Some(sig) = &item.signature
                && tally.should_suggest(sig.tier)
            {
                item.suggest = Some(tally.approved);
            }
            self.scopes.insert(key, r.cwd.clone());
            items.push(item);
        }
        for f in facts.iter().filter(|f| f.review_of.is_none()) {
            let held = approvals.iter().any(|a| a.request.job_id == Some(f.job_id));
            items.extend(job_item(f, facts, now, held));
        }
        items.retain(|i| !self.acknowledged.contains(&i.id));
        self.note_raised(&items, now);
        self.expire_shelf(&items, facts, now);
        items.retain(|i| !self.is_shelved(&i.id));
        self.open = items.clone();
        let mut items = self.apply_focus(items, now);
        self.group_batches(&mut items);
        self.flood = detect_flood(&items, now);
        if let Some(flood) = &self.flood {
            items.retain(|i| !flood.is_suppressed(&i.id));
        }
        // Forget what has gone, so a reused id starts fresh.
        self.first_seen.retain(|id, _| {
            items.iter().any(|i| &i.id == id) || self.open.iter().any(|i| &i.id == id)
        });
        self.acknowledged
            .retain(|id| facts_have(id, approvals, facts));
        rank(items)
    }

    /// Everything open at the last collection, shown or held back (by
    /// focus mode, a flood or batching) alike: shelved and acknowledged
    /// items are not. What the console ranks outcomes by.
    pub fn open(&self) -> &[AttentionItem] {
        &self.open
    }

    /// Hides an item until the run ends. Held tool calls can't be hidden:
    /// they stay until answered.
    pub fn acknowledge(&mut self, item: &AttentionItem) -> bool {
        if item.kind.is_decision() {
            return false;
        }
        self.acknowledged.insert(item.id.clone())
    }

    /// Hides every job item of `job` (looking at a job counts as reading it).
    pub fn acknowledge_job(&mut self, items: &[AttentionItem], job: JobId) {
        for i in items.iter().filter(|i| i.job == Some(job)) {
            self.acknowledge(i);
        }
    }

    /// The latest changes, newest first: job lifecycle and the notable guard
    /// decisions (denials, held calls, loop stops, routing).
    /// `extra` are changes from other sources (the hook bus).
    pub fn changes(
        &self,
        guard: &GuardService,
        jobs: &JobService,
        extra: &[Change],
        limit: usize,
    ) -> Vec<Change> {
        let decisions: Vec<&DecisionRecord> = guard.recent().collect();
        changes_from(&jobs.facts(), &decisions, extra, limit)
    }
}

/// What a decision model once said about a call it wasn't sure of.
struct Worry {
    tool: String,
    subject: String,
    worry: f64,
}

fn worry(d: &DecisionRecord) -> Option<Worry> {
    if d.source != Source::Jev || !matches!(d.outcome, Outcome::Ask | Outcome::Deny) {
        return None;
    }
    Some(Worry {
        tool: d.tool.clone(),
        subject: d.subject.clone(),
        worry: d.confidence?,
    })
}

/// How a session's earlier jobs went: the share that finished well, `0.5`
/// until there are two to judge by.
fn track_record(session: SessionId, facts: &[JobFact]) -> f64 {
    let mut done = 0u32;
    let mut bad = 0u32;
    for f in facts
        .iter()
        .filter(|f| f.session_id == session && f.review_of.is_none())
    {
        match f.status {
            JobStatus::Done => done += 1,
            JobStatus::Failed | JobStatus::Interrupted => bad += 1,
            _ => {}
        }
    }
    if done + bad < 2 {
        return 0.5;
    }
    f64::from(done) / f64::from(done + bad)
}

/// Whether an acknowledged id still refers to something that exists.
fn facts_have(id: &AttentionId, approvals: &[PendingApproval], facts: &[JobFact]) -> bool {
    let key = id.as_str();
    if let Some(rest) = key.strip_prefix("job:") {
        let job = rest.split(':').next().and_then(|n| n.parse::<i64>().ok());
        return job.is_some_and(|j| facts.iter().any(|f| f.job_id.0 == j));
    }
    approvals
        .iter()
        .any(|a| format!("approval:{}", a.request.request_id) == key)
}

/// A line to name a job by in the inbox: its prompt's first line.
fn title_of(f: &JobFact) -> String {
    f.prompt
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn job_item(f: &JobFact, facts: &[JobFact], now: Millis, held: bool) -> Option<AttentionItem> {
    let title = title_of(f);
    let session: SessionId = f.session_id;
    let ended = f.finished_at.unwrap_or(f.created_at);
    match f.status {
        JobStatus::Failed | JobStatus::Interrupted => {
            let said = f
                .result
                .as_deref()
                .and_then(|r| r.lines().find(|l| !l.trim().is_empty()));
            let error = f
                .error
                .as_deref()
                .or(said)
                .unwrap_or("ended without a result");
            Some(AttentionItem::job_failed(
                f.job_id, session, &title, error, ended,
            ))
        }
        JobStatus::Done => match &f.review {
            Some(ReviewState::Requested | ReviewState::Running { .. }) => None,
            Some(ReviewState::Done { review, job, .. })
                if review.verdict == ReviewVerdict::Concerns =>
            {
                let at = facts
                    .iter()
                    .find(|r| r.job_id == *job)
                    .and_then(|r| r.finished_at)
                    .unwrap_or(ended);
                Some(AttentionItem::review_concerns(
                    f.job_id,
                    session,
                    &title,
                    &review.concerns,
                    at,
                ))
            }
            _ => {
                let detail = f
                    .result
                    .as_deref()
                    .and_then(|r| r.lines().find(|l| !l.trim().is_empty()))
                    .unwrap_or("finished")
                    .trim()
                    .to_string();
                Some(AttentionItem::result_ready(
                    f.job_id, session, &title, &detail, ended,
                ))
            }
        },
        JobStatus::Running => {
            if f.session_state == Some(SessionState::RequiresAction) && !held {
                let since = f.last_event_at.or(f.started_at).unwrap_or(now);
                return Some(AttentionItem::needs_input(
                    f.job_id,
                    f.session_id,
                    &title,
                    since,
                ));
            }
            let last = f.last_event_at.or(f.started_at)?;
            (now - last >= STALL_AFTER_MS)
                .then(|| AttentionItem::stalled(f.job_id, session, &title, last))
        }
        JobStatus::Queued | JobStatus::Cancelled => None,
    }
}

fn changes_from(
    facts: &[JobFact],
    decisions: &[&DecisionRecord],
    extra: &[Change],
    limit: usize,
) -> Vec<Change> {
    let name_of = |job: Option<JobId>| {
        job.and_then(|j| facts.iter().find(|f| f.job_id == j))
            .map_or_else(|| "?".to_string(), |f| f.session_name.clone())
    };
    let mut out = Vec::new();
    for f in facts {
        let review = f.review_of.is_some();
        let what = if review { "review" } else { "job" };
        if let Some(at) = f.started_at {
            out.push(Change {
                at,
                job: Some(f.job_id),
                session: f.session_name.clone(),
                kind: ChangeKind::Started,
                text: format!("{what} started"),
            });
        }
        let Some(at) = f.finished_at else { continue };
        let (kind, text) = match f.status {
            JobStatus::Done if review => (ChangeKind::Reviewed, "review finished".to_string()),
            JobStatus::Done => (ChangeKind::Finished, format!("done ${:.2}", f.cost_usd)),
            JobStatus::Interrupted => (ChangeKind::Failed, format!("{what} stopped")),
            _ => (ChangeKind::Failed, format!("{what} failed")),
        };
        out.push(Change {
            at,
            job: Some(f.job_id),
            session: f.session_name.clone(),
            kind,
            text,
        });
    }
    for d in decisions {
        let kind = match d.outcome {
            Outcome::Deny => ChangeKind::Denied,
            Outcome::Ask => ChangeKind::Asked,
            Outcome::Stop => ChangeKind::Stopped,
            Outcome::Route => ChangeKind::Routed,
            Outcome::Allow | Outcome::Pass => continue,
        };
        let text = match kind {
            ChangeKind::Routed => format!("routed {}", d.subject),
            _ => format!("{} {} {}", d.outcome.as_str(), d.tool, d.subject),
        };
        out.push(Change {
            at: d.ts,
            job: d.job_id,
            session: name_of(d.job_id),
            kind,
            text: text.trim().to_string(),
        });
    }
    for c in extra {
        out.push(Change {
            session: name_of(c.job),
            ..c.clone()
        });
    }
    out.sort_by(|a, b| b.at.cmp(&a.at).then(b.job.cmp(&a.job)));
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use swarmux_domain::{AttentionKind, Priority, Review, SessionState};

    use super::*;
    use crate::ports::HookRequest;

    fn fact(id: i64, status: JobStatus) -> JobFact {
        JobFact {
            job_id: JobId(id),
            session_id: SessionId::random(),
            session_name: format!("s{id}"),
            prompt: format!("task {id}\nmore"),
            model: "sonnet".into(),
            status,
            created_at: 1_000,
            started_at: Some(2_000),
            last_event_at: Some(2_000),
            finished_at: None,
            error: None,
            result: None,
            review: None,
            review_of: None,
            cost_usd: 0.5,
            context: None,
            cache_ratio: None,
            flow: Vec::new(),
            todos_done: 0,
            todos_total: 0,
            loop_stops: 0,
            agents_running: 0,
            session_state: None,
        }
    }

    fn held(
        request_id: u64,
        job: i64,
        tool: &str,
        subject: &str,
        secs_left: u64,
    ) -> PendingApproval {
        PendingApproval {
            rule: None,
            request: HookRequest {
                request_id,
                job_id: Some(JobId(job)),
                tool_name: tool.into(),
                subject: subject.into(),
                input: String::new(),
                cwd: "C:/work".into(),
                content: String::new(),
                ..HookRequest::default()
            },
            secs_left,
        }
    }

    fn ids(items: &[AttentionItem]) -> Vec<String> {
        items.iter().map(|i| i.id.to_string()).collect()
    }

    #[test]
    fn maps_each_source_to_its_kind_and_priority() {
        let mut failed = fact(1, JobStatus::Failed);
        failed.finished_at = Some(5_000);
        failed.error = Some("claude exited with code 1".into());
        let mut done = fact(2, JobStatus::Done);
        done.finished_at = Some(6_000);
        done.result = Some("\nAll set.\nbye".into());
        let mut concerns = fact(3, JobStatus::Done);
        concerns.finished_at = Some(7_000);
        concerns.review = Some(ReviewState::Done {
            review: Review {
                verdict: ReviewVerdict::Concerns,
                concerns: vec!["no tests".into()],
                diffs_checked: 1,
            },
            job: JobId(30),
            cost_usd: 0.1,
        });
        let mut review_job = fact(30, JobStatus::Done);
        review_job.review_of = Some(JobId(3));
        review_job.finished_at = Some(8_000);
        let mut quiet = fact(4, JobStatus::Running);
        quiet.last_event_at = Some(1_000_000);
        let mut busy = fact(5, JobStatus::Running);
        busy.last_event_at = Some(1_000_000 + STALL_AFTER_MS - 1);
        let mut pending_review = fact(6, JobStatus::Done);
        pending_review.review = Some(ReviewState::Requested);
        let queued = fact(7, JobStatus::Queued);

        let facts = [
            failed,
            done,
            concerns,
            review_job,
            quiet,
            busy,
            pending_review,
            queued,
        ];
        let mut svc = AttentionService::new();
        let items = svc.collect_from(
            &[
                held(1, 5, "Bash", "git push origin main", 30),
                held(2, 5, "Edit", "src/lib.rs", 90),
            ],
            &facts,
            1_000_000 + STALL_AFTER_MS,
        );
        let got: Vec<_> = items
            .iter()
            .map(|i| (i.id.to_string(), i.kind, i.priority))
            .collect();
        assert_eq!(
            got,
            [
                ("approval:1".into(), AttentionKind::Approval, Priority::P1),
                ("approval:2".into(), AttentionKind::Approval, Priority::P2),
                (
                    "job:1:failed".into(),
                    AttentionKind::JobFailed,
                    Priority::P2
                ),
                (
                    "job:2:result".into(),
                    AttentionKind::ResultReady,
                    Priority::P3
                ),
                (
                    "job:3:review".into(),
                    AttentionKind::ReviewConcerns,
                    Priority::P3
                ),
                ("job:4:stalled".into(), AttentionKind::Stalled, Priority::P3),
            ]
        );
        assert_eq!(items[0].session, Some(facts[5].session_id));
        assert_eq!(items[0].deadline, Some(1_000_000 + STALL_AFTER_MS + 30_000));
        assert_eq!(items[2].detail, "claude exited with code 1");
        assert_eq!(items[3].detail, "All set.");
        assert_eq!(items[3].title, "task 2");
        assert_eq!(items[4].since, 8_000, "concerns date from the review");
    }

    #[test]
    fn order_is_the_same_whatever_the_input_order() {
        let mut a = fact(1, JobStatus::Failed);
        a.finished_at = Some(10);
        let mut b = fact(2, JobStatus::Done);
        b.finished_at = Some(10);
        let mut c = fact(3, JobStatus::Done);
        c.finished_at = Some(5);
        let mut forward = AttentionService::new();
        let mut backward = AttentionService::new();
        let one = forward.collect_from(&[], &[a.clone(), b.clone(), c.clone()], 100);
        let two = backward.collect_from(&[], &[c, b, a], 100);
        assert_eq!(ids(&one), ["job:1:failed", "job:3:result", "job:2:result"]);
        assert_eq!(ids(&one), ids(&two));
    }

    #[test]
    fn acknowledge_hides_job_items_but_never_approvals() {
        let mut done = fact(1, JobStatus::Done);
        done.finished_at = Some(10);
        let approval = [held(9, 1, "Bash", "cargo test", 60)];
        let mut svc = AttentionService::new();
        let items = svc.collect_from(&approval, std::slice::from_ref(&done), 100);
        assert_eq!(ids(&items), ["approval:9", "job:1:result"]);
        assert!(!svc.acknowledge(&items[0]), "a held call stays");
        assert!(svc.acknowledge(&items[1]));
        let items = svc.collect_from(&approval, std::slice::from_ref(&done), 200);
        assert_eq!(ids(&items), ["approval:9"]);
    }

    #[test]
    fn opening_a_job_acknowledges_its_items() {
        let mut failed = fact(1, JobStatus::Failed);
        failed.finished_at = Some(10);
        let mut svc = AttentionService::new();
        let items = svc.collect_from(&[], std::slice::from_ref(&failed), 100);
        svc.acknowledge_job(&items, JobId(1));
        assert!(svc.collect_from(&[], &[failed], 100).is_empty());
    }

    #[test]
    fn approval_age_is_kept_across_collections() {
        let mut svc = AttentionService::new();
        let held = [held(1, 1, "Edit", "a.rs", 60)];
        let first = svc.collect_from(&held, &[fact(1, JobStatus::Running)], 5_000);
        let later = svc.collect_from(&held, &[fact(1, JobStatus::Running)], 9_000);
        assert_eq!((first[0].since, later[0].since), (5_000, 5_000));
    }

    #[test]
    fn a_subagents_call_says_who_made_it() {
        let mut svc = AttentionService::new();
        let mut by_agent = held(1, 1, "Edit", "a.rs", 60);
        by_agent.request.agent_type = Some("Explore".into());
        by_agent.request.agent_id = Some("agent-7".into());
        let plain = held(2, 1, "Edit", "b.rs", 60);
        let items = svc.collect_from(&[by_agent, plain], &[fact(1, JobStatus::Running)], 5_000);
        let detail = |id: &str| {
            items
                .iter()
                .find(|i| i.id.to_string() == id)
                .map(|i| i.detail.clone())
                .unwrap()
        };
        assert_eq!(
            detail("approval:1"),
            "subagent Explore (agent-7) · waiting for your answer"
        );
        assert_eq!(detail("approval:2"), "waiting for your answer");
    }

    #[test]
    fn changes_list_lifecycle_and_notable_decisions_newest_first() {
        let mut done = fact(1, JobStatus::Done);
        done.finished_at = Some(9_000);
        let running = fact(2, JobStatus::Running);
        let decision = |outcome, ts, tool: &str| DecisionRecord {
            job_id: Some(JobId(2)),
            tool: tool.into(),
            subject: "rm -rf x".into(),
            outcome,
            rule: None,
            latency_us: 1,
            ts,
            source: crate::Source::Guard,
            confidence: None,
            extra: crate::DecisionExtra::default(),
        };
        let allow = decision(Outcome::Allow, 9_500, "Read");
        let deny = decision(Outcome::Deny, 8_000, "Bash");
        let rows = changes_from(&[done, running], &[&allow, &deny], &[], 10);
        let text: Vec<_> = rows
            .iter()
            .map(|c| format!("{} {} {}", c.at, c.session, c.text))
            .collect();
        assert_eq!(
            text,
            [
                "9000 s1 done $0.50",
                "8000 s2 deny Bash rm -rf x",
                "2000 s2 job started",
                "2000 s1 job started"
            ]
        );
        assert_eq!(changes_from(&[], &[&deny], &[], 0).len(), 0);
    }

    #[test]
    fn requires_action_without_a_held_call_is_a_p2_question() {
        let mut waiting = fact(1, JobStatus::Running);
        waiting.session_state = Some(SessionState::RequiresAction);
        let mut explained = fact(2, JobStatus::Running);
        explained.session_state = Some(SessionState::RequiresAction);
        let mut busy = fact(3, JobStatus::Running);
        busy.session_state = Some(SessionState::Running);
        let held = vec![held(7, 2, "Bash", "make", 60)];
        let items = AttentionService::new().collect_from(&held, &[waiting, explained, busy], 5_000);
        let got: Vec<_> = items
            .iter()
            .map(|i| (i.id.to_string(), i.kind, i.priority))
            .collect();
        assert_eq!(
            got,
            [
                ("approval:7".into(), AttentionKind::Approval, Priority::P2),
                (
                    "job:1:question".into(),
                    AttentionKind::Question,
                    Priority::P2
                ),
            ]
        );
    }

    #[test]
    fn extra_changes_are_named_and_merged() {
        let f = fact(1, JobStatus::Running);
        let extra = Change {
            at: 9_000,
            job: Some(JobId(1)),
            session: String::new(),
            kind: ChangeKind::Denied,
            text: "classifier denied Bash".into(),
        };
        let rows = changes_from(&[f], &[], &[extra], 10);
        assert_eq!(rows[0].session, "s1");
        assert_eq!(rows[0].kind, ChangeKind::Denied);
    }
}
