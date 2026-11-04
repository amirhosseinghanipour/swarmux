//! The ledger use cases: recording what swarmux observed or decided, and
//! checking that nothing recorded was changed since.
//!
//! Recording happens through [`LedgeredJobs`] and [`LedgeredDecisions`]:
//! they wrap the job and decision repositories, so every job status, job
//! event, review and decision that is stored is also appended to the
//! ledger, on the same thread, right after it is stored. The `events` and
//! `decisions` tables stay the fast indexes; the ledger is the record.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use swarmux_domain::ledger::{
    ChainBreak, ChainVerifier, Hash, LedgerDraft, LedgerEntry, LedgerKind, Payload,
};
use swarmux_domain::{Job, JobId, JobResult, JobStatus, Millis, ReviewState, SessionId};

use crate::ports::{Clock, DecisionRepository, JobRepository, NewJob, StoredEvent};
use crate::{AppResult, DecisionRecord, JobSummary};

/// The append-only, hash-chained log.
///
/// Only swarmux's own use cases hold one; the agent runner never does.
pub trait Ledger {
    /// Chains `draft` after the head and stores it. On an empty ledger the
    /// genesis entry is stored first.
    fn append(&self, draft: LedgerDraft) -> AppResult<LedgerEntry>;
    /// The newest entry.
    fn head(&self) -> AppResult<Option<LedgerEntry>>;
    /// Up to `limit` entries with `seq >= from_seq`, in `seq` order.
    fn range(&self, from_seq: u64, limit: usize) -> AppResult<Vec<LedgerEntry>>;
}

/// Who appends entries for swarmux itself.
const SWARMUX: &str = "swarmux";
/// Entries read per page while verifying.
const VERIFY_PAGE: usize = 1000;

/// What [`verify_all`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyReport {
    /// Entries that verified before the end or the break.
    pub entries: u64,
    /// The stored head, verified or not.
    pub head: Option<(u64, Hash)>,
    /// The first entry where the chain breaks.
    pub broken: Option<ChainBreak>,
}

impl VerifyReport {
    pub fn is_ok(&self) -> bool {
        self.broken.is_none()
    }
}

/// Walks the whole ledger in pages and checks the chain.
pub fn verify_all(ledger: &dyn Ledger) -> AppResult<VerifyReport> {
    verify_in_pages(ledger, VERIFY_PAGE)
}

fn verify_in_pages(ledger: &dyn Ledger, page: usize) -> AppResult<VerifyReport> {
    let head = ledger.head()?.map(|e| (e.seq, e.hash));
    let mut v = ChainVerifier::default();
    let mut from = 1;
    loop {
        let entries = ledger.range(from, page)?;
        let Some(last) = entries.last() else { break };
        from = last.seq + 1;
        for e in &entries {
            if let Err(broken) = v.push(e) {
                return Ok(VerifyReport {
                    entries: v.checked(),
                    head,
                    broken: Some(broken),
                });
            }
        }
        if entries.len() < page {
            break;
        }
    }
    Ok(VerifyReport {
        entries: v.checked(),
        head,
        broken: None,
    })
}

/// The newest `n` entries, oldest first.
pub fn tail(ledger: &dyn Ledger, n: usize) -> AppResult<Vec<LedgerEntry>> {
    let Some(head) = ledger.head()? else {
        return Ok(Vec::new());
    };
    let from = head.seq.saturating_sub(n as u64).saturating_add(1);
    ledger.range(from, n)
}

/// Turns what swarmux stores into ledger entries.
/// One watchdog entry, as [`LedgerService::record_watchdog`] writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatchdogEntry<'a> {
    pub job: JobId,
    pub session: Option<SessionId>,
    pub event: &'a str,
    pub alarm: &'a str,
    pub rung: Option<&'a str>,
    pub detail: &'a str,
    pub next_job: Option<JobId>,
}

pub struct LedgerService {
    ledger: Rc<dyn Ledger>,
    clock: Rc<dyn Clock>,
}

impl LedgerService {
    pub fn new(ledger: Rc<dyn Ledger>, clock: Rc<dyn Clock>) -> LedgerService {
        LedgerService { ledger, clock }
    }

    pub fn ledger(&self) -> &dyn Ledger {
        &*self.ledger
    }

    fn append(
        &self,
        ts: Millis,
        kind: LedgerKind,
        actor: &str,
        job: Option<JobId>,
        session: Option<SessionId>,
        payload: Payload,
    ) -> AppResult<()> {
        self.append_entry(ts, kind, actor, job, session, payload)
            .map(drop)
    }

    /// Like `append`, and returns the entry's `seq`.
    fn append_entry(
        &self,
        ts: Millis,
        kind: LedgerKind,
        actor: &str,
        job: Option<JobId>,
        session: Option<SessionId>,
        payload: Payload,
    ) -> AppResult<u64> {
        self.ledger
            .append(LedgerDraft {
                ts,
                kind,
                actor: actor.to_string(),
                job,
                session: session.map(|s| s.to_string()),
                payload_json: payload.build(),
            })
            .map(|e| e.seq)
    }

    /// A job's worktree was created (`action` `created`) or reclaimed
    /// (`reclaimed`, with the commit that saved its last files in `detail`).
    pub fn record_worktree(
        &self,
        w: &swarmux_domain::WorktreeRecord,
        action: &str,
        detail: Option<&str>,
        now: Millis,
    ) -> AppResult<()> {
        let payload = Payload::new()
            .str("action", action)
            .text("path", &w.path.display().to_string())
            .text("repo", &w.repo.display().to_string())
            .str("branch", &w.branch)
            .int("slot", i64::from(w.slot))
            .int("port_base", i64::from(w.port_base))
            .opt_str("saved_commit", detail);
        self.append(
            now,
            LedgerKind::Worktree,
            SWARMUX,
            Some(w.job),
            Some(w.session),
            payload,
        )
    }

    /// A mission went from `from` to its current state.
    pub fn record_mission(
        &self,
        m: &swarmux_domain::mission::Mission,
        from: swarmux_domain::mission::MissionState,
        now: Millis,
    ) -> AppResult<()> {
        let payload = Payload::new()
            .str("mission", &m.slug)
            .str("from", from.as_str())
            .str("to", m.state.as_str())
            .text("title", &m.contract.title)
            .num("spent_usd", m.spend.total())
            .opt_str("note", m.note.as_deref());
        self.append(now, LedgerKind::Mission, SWARMUX, m.job, m.session, payload)
    }

    /// A mission's evidence bundle was sealed; returns the entry's `seq`.
    pub fn record_evidence(
        &self,
        slug: &str,
        job: Option<JobId>,
        digest: &str,
        head_sha: &str,
        to_seq: u64,
        now: Millis,
    ) -> AppResult<u64> {
        let payload = Payload::new()
            .str("mission", slug)
            .str("digest", digest)
            .str("head_sha", head_sha)
            .int("to_seq", i64::try_from(to_seq).unwrap_or(i64::MAX));
        self.append_entry(now, LedgerKind::Evidence, SWARMUX, job, None, payload)
    }

    /// An outcome label attached to a delivered mission.
    pub fn record_label(
        &self,
        slug: &str,
        job: Option<JobId>,
        label: &swarmux_domain::evidence::Label,
        now: Millis,
    ) -> AppResult<()> {
        let payload = Payload::new()
            .str("mission", slug)
            .str("label", label.kind.as_str())
            .int("pr", i64::try_from(label.pr).unwrap_or(i64::MAX))
            .int("at", label.at);
        self.append(now, LedgerKind::Label, SWARMUX, job, None, payload)
    }

    /// A checkpoint; returns the entry's `seq`.
    pub fn record_checkpoint(
        &self,
        c: &swarmux_domain::Checkpoint,
        handoff_file: Option<&str>,
        session: Option<SessionId>,
    ) -> AppResult<u64> {
        let payload = Payload::new()
            .str("reason", c.reason.as_str())
            .str("claude_session", &c.session_id)
            .str("commit", &c.commit)
            .text("worktree", &c.worktree.display().to_string())
            .str("branch", &c.branch)
            .opt_str("handoff_file", handoff_file)
            .json(
                "handoff",
                &c.handoff
                    .as_ref()
                    .map_or_else(|| "null".to_string(), |h| h.to_json()),
            );
        self.append_entry(
            c.at,
            LedgerKind::Checkpoint,
            SWARMUX,
            Some(c.job),
            session,
            payload,
        )
    }

    pub fn record_job_queued(&self, id: JobId, job: &NewJob, now: Millis) -> AppResult<()> {
        let payload = Payload::new()
            .text("prompt", &job.prompt)
            .opt_int("prompt_id", job.prompt_id.map(|p| p.0))
            .str("model", &job.model)
            .int("priority", i64::from(job.priority))
            .bool("fork", job.fork)
            .bool("wants_review", job.wants_review)
            .opt_int("review_of", job.review_of.map(|j| j.0));
        self.append(
            now,
            LedgerKind::JobQueued,
            SWARMUX,
            Some(id),
            Some(job.session_id),
            payload,
        )
    }

    pub fn record_job_started(
        &self,
        id: JobId,
        session: Option<SessionId>,
        now: Millis,
    ) -> AppResult<()> {
        let payload = Payload::new().str("status", JobStatus::Running.as_str());
        self.append(
            now,
            LedgerKind::JobStarted,
            SWARMUX,
            Some(id),
            session,
            payload,
        )
    }

    /// A job reached `status`. `revised` marks new final numbers for a job
    /// that had already finished.
    pub fn record_job_finished(
        &self,
        id: JobId,
        session: Option<SessionId>,
        status: JobStatus,
        result: Option<&JobResult>,
        revised: bool,
        now: Millis,
    ) -> AppResult<()> {
        let mut payload = Payload::new().str("status", status.as_str());
        if let Some(r) = result {
            payload = payload
                .str("outcome", &r.outcome)
                .opt_str("error", r.error.as_deref())
                .num("cost_usd", r.cost_usd)
                .int("duration_ms", saturating(r.duration_ms))
                .int("tokens_in", saturating(r.tokens_in))
                .int("tokens_out", saturating(r.tokens_out))
                .int("cache_read", saturating(r.cache_read))
                .text("answer", &r.text);
        }
        if revised {
            payload = payload.bool("revised", true);
        }
        self.append(
            now,
            LedgerKind::JobFinished,
            SWARMUX,
            Some(id),
            session,
            payload,
        )
    }

    /// Jobs a previous run left running were marked interrupted.
    pub fn record_interrupted(&self, count: usize, now: Millis) -> AppResult<()> {
        let payload = Payload::new()
            .str("status", JobStatus::Interrupted.as_str())
            .int("count", count as i64);
        self.append(now, LedgerKind::JobFinished, SWARMUX, None, None, payload)
    }

    /// An agent event as stored for the job (`summary` is its log line).
    pub fn record_agent_event(
        &self,
        id: JobId,
        session: Option<SessionId>,
        event: &str,
        actor: &str,
        summary: &str,
        now: Millis,
    ) -> AppResult<()> {
        let payload = Payload::new().str("event", event).text("summary", summary);
        self.append(
            now,
            LedgerKind::AgentEvent,
            actor,
            Some(id),
            session,
            payload,
        )
    }

    /// A lifecycle event reported through a Claude Code hook.
    pub fn record_hook_event(
        &self,
        n: &crate::hookbus::HookNotice,
        session: Option<SessionId>,
        now: Millis,
    ) -> AppResult<()> {
        let payload = Payload::new()
            .str("event", n.event.as_str())
            .opt_str("tool", n.tool.as_deref())
            .str("session", &n.session)
            .text("summary", &n.summary)
            .str("input_sha256", &n.input_sha256)
            .int("input_len", saturating(n.input_len as u64));
        self.append(now, LedgerKind::AgentEvent, "hook", n.job, session, payload)
    }

    /// A done-gate verdict, or a manual run of a repository's checks.
    pub fn record_gate(
        &self,
        r: &crate::gate::GateRecord,
        session: Option<SessionId>,
        now: Millis,
    ) -> AppResult<()> {
        let runs = r
            .findings
            .runs
            .iter()
            .map(|c| {
                Payload::new()
                    .str("name", &c.name)
                    .text("command", &c.command)
                    .opt_int("exit", c.exit.map(i64::from))
                    .bool("timed_out", c.timed_out)
                    .bool("flaky", c.flaky)
                    .int("duration_ms", saturating(c.duration_ms))
                    .opt_str("spawn_error", c.spawn_error.as_deref())
                    .text(
                        "first_failure",
                        &c.failure_excerpt().join(
                            "
",
                        ),
                    )
            })
            .collect();
        let pins: Vec<String> = r
            .findings
            .pins
            .iter()
            .map(|p| format!("{} {}", p.verb(), p.path()))
            .collect();
        // A manual run blocks nobody: it passed or failed.
        let outcome = match (r.event, r.findings.clean()) {
            (Some(_), _) => r.outcome.as_str(),
            (None, true) => "pass",
            (None, false) => "fail",
        };
        let payload = Payload::new()
            .opt_str("event", r.event.map(|e| e.as_str()))
            .str("session", &r.session)
            .str("outcome", outcome)
            .int("blocks", i64::from(r.blocks))
            .int("max_blocks", i64::from(r.max_blocks))
            .text("root", &r.root)
            .str("origin", r.origin.as_str())
            .objects("checks", runs)
            .strs("protected_changed", &pins)
            .strs("evidence_gaps", &r.findings.gaps)
            .strs("tampering", &r.findings.tampering)
            .text("digest", &r.digest)
            .opt_str("problem", r.problem.as_deref());
        // Hidden checks are counted, never named.
        let payload = match r.findings.hidden {
            Some(h) => payload.json(
                "hidden",
                &Payload::new()
                    .int("ran", i64::from(h.ran))
                    .int("passed", i64::from(h.passed))
                    .build(),
            ),
            None => payload,
        };
        let actor = if r.event.is_some() { "gate" } else { "cli" };
        self.append(now, LedgerKind::GateVerdict, actor, r.job, session, payload)
    }

    /// What the watchdog saw or did for `job`: an `alarm`, a `rung` taken,
    /// a rung's `reply` (a judgement or diagnosis), work that `settled` or
    /// was `rescued`. `next_job` is the job a new session runs as.
    pub fn record_watchdog(&self, w: &WatchdogEntry<'_>, now: Millis) -> AppResult<()> {
        let payload = Payload::new()
            .str("event", w.event)
            .str("alarm", w.alarm)
            .opt_str("rung", w.rung)
            .text("detail", w.detail)
            .opt_int("next_job", w.next_job.map(|j| j.0));
        self.append(
            now,
            LedgerKind::Watchdog,
            "watchdog",
            Some(w.job),
            w.session,
            payload,
        )
    }

    pub fn record_decision(&self, d: &DecisionRecord) -> AppResult<()> {
        let payload = Payload::new()
            .str("source", d.source.as_str())
            .str("outcome", d.outcome.as_str())
            .str("tool", &d.tool)
            .text("subject", &d.subject)
            .opt_str("rule", d.rule.as_deref())
            .int("latency_us", saturating(d.latency_us))
            .opt_num("confidence", d.confidence);
        self.append(
            d.ts,
            LedgerKind::GuardDecision,
            d.source.as_str(),
            d.job_id,
            None,
            payload,
        )
    }

    pub fn record_review(
        &self,
        id: JobId,
        session: Option<SessionId>,
        review: &ReviewState,
    ) -> AppResult<()> {
        let payload = Payload::new().json("review", &review.encode());
        self.append(
            self.clock.now(),
            LedgerKind::ReviewVerdict,
            "reviewer",
            Some(id),
            session,
            payload,
        )
    }
}

fn saturating(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// A job repository that also records every change in the ledger.
pub struct LedgeredJobs {
    inner: Rc<dyn JobRepository>,
    ledger: Rc<LedgerService>,
    /// Which session each job of this run belongs to.
    sessions: RefCell<HashMap<JobId, SessionId>>,
}

impl LedgeredJobs {
    pub fn new(inner: Rc<dyn JobRepository>, ledger: Rc<LedgerService>) -> LedgeredJobs {
        LedgeredJobs {
            inner,
            ledger,
            sessions: RefCell::default(),
        }
    }

    fn session_of(&self, id: JobId) -> AppResult<Option<SessionId>> {
        if let Some(s) = self.sessions.borrow().get(&id) {
            return Ok(Some(*s));
        }
        let session = self.inner.get(id)?.map(|j| j.session_id);
        if let Some(s) = session {
            self.sessions.borrow_mut().insert(id, s);
        }
        Ok(session)
    }
}

impl JobRepository for LedgeredJobs {
    fn insert(&self, job: &NewJob, now: Millis) -> AppResult<JobId> {
        let id = self.inner.insert(job, now)?;
        self.sessions.borrow_mut().insert(id, job.session_id);
        self.ledger.record_job_queued(id, job, now)?;
        Ok(id)
    }

    fn get(&self, id: JobId) -> AppResult<Option<Job>> {
        self.inner.get(id)
    }

    fn queued(&self) -> AppResult<Vec<Job>> {
        self.inner.queued()
    }

    fn set_status(&self, id: JobId, status: JobStatus, now: Millis) -> AppResult<()> {
        self.inner.set_status(id, status, now)?;
        let session = self.session_of(id)?;
        if status == JobStatus::Running {
            self.ledger.record_job_started(id, session, now)
        } else if status.is_finished() {
            let result = self.inner.result(id)?;
            self.ledger
                .record_job_finished(id, session, status, result.as_ref(), false, now)
        } else {
            Ok(())
        }
    }

    fn status(&self, id: JobId) -> AppResult<JobStatus> {
        self.inner.status(id)
    }

    fn set_priority(&self, id: JobId, priority: i32) -> AppResult<()> {
        self.inner.set_priority(id, priority)
    }

    fn set_session(&self, id: JobId, session: SessionId) -> AppResult<()> {
        self.inner.set_session(id, session)?;
        self.sessions.borrow_mut().insert(id, session);
        Ok(())
    }

    fn save_result(&self, id: JobId, result: &JobResult) -> AppResult<()> {
        // Before a job ends its result is recorded with the final status;
        // after, a new result revises the finished job's numbers.
        let status = self.inner.status(id)?;
        self.inner.save_result(id, result)?;
        if status.is_finished() {
            let session = self.session_of(id)?;
            let now = self.ledger.clock.now();
            self.ledger
                .record_job_finished(id, session, status, Some(result), true, now)?;
        }
        Ok(())
    }

    fn result(&self, id: JobId) -> AppResult<Option<JobResult>> {
        self.inner.result(id)
    }

    fn append_event(
        &self,
        id: JobId,
        kind: &str,
        actor: &str,
        payload: &str,
        now: Millis,
    ) -> AppResult<()> {
        self.inner.append_event(id, kind, actor, payload, now)?;
        let session = self.session_of(id)?;
        self.ledger
            .record_agent_event(id, session, kind, actor, payload, now)
    }

    fn events(&self, id: JobId) -> AppResult<Vec<StoredEvent>> {
        self.inner.events(id)
    }

    fn interrupt_running(&self, now: Millis) -> AppResult<usize> {
        let n = self.inner.interrupt_running(now)?;
        if n > 0 {
            self.ledger.record_interrupted(n, now)?;
        }
        Ok(n)
    }

    fn summaries(&self, limit: usize) -> AppResult<Vec<JobSummary>> {
        self.inner.summaries(limit)
    }

    fn summary(&self, id: JobId) -> AppResult<Option<JobSummary>> {
        self.inner.summary(id)
    }

    fn set_review(&self, id: JobId, review: &ReviewState) -> AppResult<()> {
        self.inner.set_review(id, review)?;
        let session = self.session_of(id)?;
        self.ledger.record_review(id, session, review)
    }

    fn review(&self, id: JobId) -> AppResult<Option<ReviewState>> {
        self.inner.review(id)
    }
}

/// A decision log that also records every decision in the ledger.
pub struct LedgeredDecisions {
    inner: Rc<dyn DecisionRepository>,
    ledger: Rc<LedgerService>,
}

impl LedgeredDecisions {
    pub fn new(inner: Rc<dyn DecisionRepository>, ledger: Rc<LedgerService>) -> LedgeredDecisions {
        LedgeredDecisions { inner, ledger }
    }
}

impl DecisionRepository for LedgeredDecisions {
    fn record(&self, decision: &DecisionRecord) -> AppResult<()> {
        self.inner.record(decision)?;
        self.ledger.record_decision(decision)
    }

    fn recent_decisions(&self, limit: usize) -> AppResult<Vec<DecisionRecord>> {
        self.inner.recent_decisions(limit)
    }
}

#[cfg(test)]
mod tests;
