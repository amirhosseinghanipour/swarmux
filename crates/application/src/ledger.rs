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

use swarmux_domain::gate::{GateOrigin, GateOutcome};
use swarmux_domain::ledger::{
    ChainBreak, ChainVerifier, Hash, LedgerDraft, LedgerEntry, LedgerKind, Payload,
};
use swarmux_domain::ledger::{hex, sha256};
use swarmux_domain::{
    Job, JobId, JobResult, JobStatus, Millis, ReviewState, SessionId, StoredRule,
};

use crate::ports::{
    Clock, DecisionRepository, FallbackRecord, FallbackSpool, JobRepository, NewJob, RuleStore,
    StoredEvent,
};
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

/// One note of the operator to an agent, as [`LedgerService::record_operator_note`]
/// writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperatorNote<'a> {
    /// `send_back` or `deny_with_note`.
    pub action: &'a str,
    pub note: &'a str,
    /// The job the note is about: the one sent back, or the one whose call
    /// was held.
    pub job: Option<JobId>,
    pub session: Option<SessionId>,
    /// Where the job or its mission stood: `escalated`, `verifying`,
    /// `failed`, `done`, `held` ...
    pub outcome: &'a str,
    pub mission: Option<&'a str>,
    /// The job the agent carries on as.
    pub resumed_as: Option<JobId>,
    /// The held call's request, and the call itself in a line.
    pub request: Option<u64>,
    pub call: Option<&'a str>,
    /// The gate judges the agent's next claim, the checks run again.
    pub gate_reruns: bool,
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

    /// The time the entries are stamped by.
    pub fn now(&self) -> Millis {
        self.clock.now()
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

    /// A knowledge item changed, was delivered, or a rule acted (or would
    /// have); returns the entry's `seq`.
    pub fn record_knowledge(
        &self,
        job: Option<JobId>,
        payload: Payload,
        now: Millis,
    ) -> AppResult<u64> {
        self.append_entry(now, LedgerKind::Knowledge, SWARMUX, job, None, payload)
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

    /// Something happened to a mission's plan: `proposed`, `approved`,
    /// `rejected` (with the note in `detail`), `cancelled`, `failed`, or
    /// the build compared with it (`drift`, `on_plan`). `digest` names the
    /// plan an approval vouches for.
    pub fn record_plan(
        &self,
        m: &swarmux_domain::mission::Mission,
        event: &str,
        detail: &str,
        digest: Option<&str>,
        tasks: usize,
        now: Millis,
    ) -> AppResult<()> {
        let payload = Payload::new()
            .str("mission", &m.slug)
            .str("plan", event)
            .text("detail", detail)
            .opt_str("digest", digest)
            .int("tasks", i64::try_from(tasks).unwrap_or(i64::MAX));
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

    /// Something happened to a follow-up: `found`, `merged`, `approved`,
    /// `auto-approved`, `declined`, `deferred`, `due` or `spot_check`.
    /// `by` is `owner`, `order:<id>` or `swarmux`.
    #[allow(clippy::too_many_arguments)]
    pub fn record_followup(
        &self,
        event: &str,
        id: i64,
        kind: &str,
        by: &str,
        detail: &str,
        job: Option<JobId>,
        cost_usd: f64,
    ) -> AppResult<()> {
        let payload = Payload::new()
            .str("event", event)
            .int("followup", id)
            .str("kind", kind)
            .str("by", by)
            .text("detail", detail)
            .num("cost_usd", cost_usd);
        self.append(
            self.clock.now(),
            LedgerKind::Followup,
            SWARMUX,
            job,
            None,
            payload,
        )
    }

    /// The small-model call that listed a finished job's follow-ups, and
    /// what it cost.
    pub fn record_followup_extraction(
        &self,
        job: JobId,
        detail: &str,
        cost_usd: f64,
    ) -> AppResult<()> {
        let payload = Payload::new()
            .str("event", "extract")
            .text("detail", detail)
            .num("cost_usd", cost_usd);
        self.append(
            self.clock.now(),
            LedgerKind::Followup,
            SWARMUX,
            Some(job),
            None,
            payload,
        )
    }

    /// An owner brief was sent.
    pub fn record_brief(&self, cards: usize, finished: usize, met: usize) -> AppResult<()> {
        let payload = Payload::new()
            .str("event", "brief")
            .int("cards", i64::try_from(cards).unwrap_or(i64::MAX))
            .int("finished", i64::try_from(finished).unwrap_or(i64::MAX))
            .int("met_checks", i64::try_from(met).unwrap_or(i64::MAX));
        self.append(
            self.clock.now(),
            LedgerKind::Followup,
            SWARMUX,
            None,
            None,
            payload,
        )
    }

    /// The operator told an agent something: `note` sent a job's work back
    /// (`action` `send_back`), or went with the denial of a held call
    /// (`deny_with_note`). Who (the actor is `operator`), when, the text in
    /// full up to the ledger's cap, the job, what state it was in, and what
    /// happened next. Returns the entry's `seq`.
    pub fn record_operator_note(&self, n: &OperatorNote<'_>, now: Millis) -> AppResult<u64> {
        let payload = Payload::new()
            .str("action", n.action)
            .text("note", n.note)
            .opt_int("job", n.job.map(|j| j.0))
            .str("outcome", n.outcome)
            .opt_str("mission", n.mission)
            .opt_int("resumed_as", n.resumed_as.map(|j| j.0))
            .opt_int("request", n.request.and_then(|r| i64::try_from(r).ok()))
            .opt_str("call", n.call)
            .bool("gate_reruns", n.gate_reruns);
        self.append_entry(
            now,
            LedgerKind::Operator,
            "operator",
            n.job,
            n.session,
            payload,
        )
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
        // A manual run blocks nobody: it passed, failed, or found nothing
        // wrong but could not vouch for the work (escalate): never "pass".
        let outcome = match (r.event, r.findings.clean(), r.outcome) {
            (Some(_), _, o) => o.as_str(),
            (None, true, GateOutcome::Pass) => "pass",
            (None, true, _) => "escalate",
            (None, false, _) => "fail",
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
            .opt_str("problem", r.problem.as_deref())
            .strs("substrate", &r.findings.substrate)
            .strs("dependencies", &r.findings.dependencies)
            .opt_str("ran_in", r.findings.ran_in.map(|x| x.as_str()))
            .strs("planned", &r.findings.planned)
            .objects(
                "left_out",
                r.findings
                    .left_out
                    .iter()
                    .map(|(name, why)| Payload::new().str("name", name).text("why", why))
                    .collect(),
            )
            .strs("incomplete", &r.findings.incomplete())
            .strs("warnings", &r.findings.warnings)
            .bool("unattended", r.findings.unattended)
            .str("policy_sha256", &r.findings.policy_sha256)
            .str(
                "policy_tier",
                match r.origin {
                    GateOrigin::File => "project",
                    GateOrigin::Detected => "core",
                },
            )
            .text(
                "policy_source",
                &match r.origin {
                    GateOrigin::File => format!(
                        "{}/.swarmux/gates.toml",
                        r.root.trim_end_matches(['/', '\\'])
                    ),
                    GateOrigin::Detected => {
                        "checks guessed from the repository's files".to_string()
                    }
                },
            );
        // What the verdict could not say about the hidden checks, and how
        // the base suite and the revert probe went, for the bundle.
        let payload = match &r.findings.hidden_not_run {
            Some(h) => payload.json(
                "hidden_not_run",
                &Payload::new()
                    .int("planned", i64::from(h.planned))
                    .text("reason", &h.reason)
                    .build(),
            ),
            None => payload,
        };
        let payload = match &r.findings.base_suite {
            Some(b) => payload.json(
                "base_suite",
                &Payload::new()
                    .str("outcome", if b.clean() { "pass" } else { "fail" })
                    .strs("restored", &b.restored)
                    .text("verdict", &b.text())
                    .build(),
            ),
            None => payload,
        };
        let payload = match &r.findings.revert_probe {
            Some(p) => payload.json(
                "revert_probe",
                &Payload::new()
                    .bool("proved", p.proved)
                    .text("detail", &p.detail)
                    .build(),
            ),
            None => payload,
        };
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
        self.append(now, LedgerKind::GateVerdict, actor, r.job, session, payload)?;
        // The base-suite step has its own entry: what was put back, what
        // failed against it.
        let Some(base) = &r.findings.base_suite else {
            return Ok(());
        };
        let failed: Vec<Payload> = base
            .failed
            .iter()
            .map(|(name, line)| {
                Payload::new()
                    .str("check", name)
                    .text("first_failure", line)
            })
            .collect();
        let payload = Payload::new()
            .str("outcome", if base.clean() { "pass" } else { "fail" })
            .strs("restored", &base.restored)
            .objects("failed", failed)
            .text("verdict", &base.text());
        self.append(now, LedgerKind::BaseSuite, actor, r.job, session, payload)
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
        let rule_sha256 = d.rule.as_deref().map(|r| hex(&sha256(r.as_bytes())));
        let payload = Payload::new()
            .str("source", d.source.as_str())
            .str("outcome", d.outcome.as_str())
            .str("tool", &d.tool)
            .text("subject", &d.subject)
            .opt_str("rule", d.rule.as_deref())
            .opt_str("rule_sha256", rule_sha256.as_deref())
            .int("latency_us", saturating(d.latency_us))
            .opt_num("confidence", d.confidence)
            .opt_str("resolver", d.extra.resolver.as_deref())
            .opt_str("agent_id", d.extra.agent_id.as_deref())
            .opt_str("agent_type", d.extra.agent_type.as_deref())
            .opt_str("policy_sha256", d.extra.policy_sha256.as_deref())
            .opt_str("rule_tier", d.extra.rule_tier.as_deref())
            .opt_str("rule_source", d.extra.rule_source.as_deref());
        self.append(
            d.ts,
            LedgerKind::GuardDecision,
            d.source.as_str(),
            d.job_id,
            None,
            payload,
        )
    }

    /// An allow rule was added (`added`) or taken away (`removed`, or
    /// `expired` when it was pruned): what it says, where it applies, who
    /// made it and when it stops, and the hash of the rule as kept.
    pub fn record_rule(&self, event: &str, rule: &StoredRule, now: Millis) -> AppResult<()> {
        let kept = format!(
            "{} {} {} {}",
            rule.text,
            rule.scope.as_str(),
            rule.root.as_deref().unwrap_or(""),
            rule.expires_at.map_or(String::new(), |e| e.to_string())
        );
        let payload = Payload::new()
            .str("event", event)
            .str("id", &rule.id)
            .text("rule", &rule.text)
            .str("scope", rule.scope.as_str())
            .opt_str("root", rule.root.as_deref())
            .str("source", rule.source.as_str())
            .int("created_at", rule.created_at)
            .opt_int("expires_at", rule.expires_at)
            .str("rule_sha256", &hex(&sha256(kept.as_bytes())));
        self.append(now, LedgerKind::Policy, "operator", None, None, payload)
    }

    /// The runs of checks in the last `limit` ledger entries: for each
    /// gate verdict the repository root and each check's name and whether
    /// it failed and then passed. Oldest first. A ledger that cannot be
    /// read gives nothing: this seeds a flake history, not a verdict.
    pub fn recent_gate_runs(&self, limit: usize) -> Vec<(String, Vec<(String, bool)>)> {
        let Ok(entries) = tail(self.ledger(), limit) else {
            return Vec::new();
        };
        entries
            .iter()
            .filter(|e| e.draft.kind == LedgerKind::GateVerdict)
            .filter_map(|e| swarmux_domain::evidence::check_flakes(&e.draft.payload_json))
            .collect()
    }

    /// The gate's canary ran (`swarmux gate selftest`): each known-bad
    /// change and whether the gate refused it, and the hash of the gate
    /// configuration that judged.
    pub fn record_gate_selftest(
        &self,
        root: &str,
        probes: &[(String, bool, String)],
        skipped: &[(String, String)],
        policy_sha256: &str,
        now: Millis,
    ) -> AppResult<()> {
        let passed = !probes.is_empty() && probes.iter().all(|p| p.1);
        let objects = probes
            .iter()
            .map(|(name, caught, detail)| {
                Payload::new()
                    .text("name", name)
                    .bool("caught", *caught)
                    .text("detail", detail)
            })
            .collect();
        let skipped_objects = skipped
            .iter()
            .map(|(name, why)| Payload::new().text("name", name).text("why", why))
            .collect();
        let payload = Payload::new()
            .str("event", "gate_selftest")
            .str("outcome", if passed { "pass" } else { "fail" })
            .text("root", root)
            .objects("probes", objects)
            .objects("skipped", skipped_objects)
            .str("policy_sha256", policy_sha256);
        self.append(now, LedgerKind::Policy, "operator", None, None, payload)
    }

    /// Hidden checks that were kept under a repository's folder name were
    /// moved under its identity key (see `holdout`): which folder, to which
    /// key.
    pub fn record_holdout_migrated(&self, from: &str, to: &str, now: Millis) -> AppResult<()> {
        let payload = Payload::new()
            .str("event", "holdout_migrated")
            .str("from", from)
            .str("to", to);
        self.append(now, LedgerKind::Policy, "operator", None, None, payload)
    }

    /// The owner trusted a repository: which one, the hash of every file
    /// (and guessed command) the grant covers, and the commands it lets run.
    /// Trust is read back from these entries, newest wins.
    pub fn record_trust_granted(
        &self,
        record: &swarmux_domain::trust::TrustRecord,
        supplied: &swarmux_domain::trust::Supplied,
        now: Millis,
    ) -> AppResult<()> {
        let files = record
            .entries
            .iter()
            .map(|e| {
                Payload::new()
                    .text("path", &e.path)
                    .str("sha256", &e.sha256)
            })
            .collect();
        let commands: Vec<String> = supplied.commands.iter().map(|c| c.line.clone()).collect();
        let covered: String = record
            .entries
            .iter()
            .map(|e| format!("{} {}\n", e.path, e.sha256))
            .collect();
        let payload = Payload::new()
            .str("event", "trust_granted")
            .str("repo_id", &record.repo_id)
            .text("root", &record.root)
            .objects("file_hashes", files)
            .strs("commands", &commands)
            .strs("loosens", &supplied.loosens)
            .str("policy_sha256", &hex(&sha256(covered.as_bytes())));
        self.append(now, LedgerKind::Policy, "operator", None, None, payload)
    }

    /// The owner took a repository's trust back.
    pub fn record_trust_revoked(&self, repo_id: &str, root: &str, now: Millis) -> AppResult<()> {
        let payload = Payload::new()
            .str("event", "trust_revoked")
            .str("repo_id", repo_id)
            .text("root", root);
        self.append(now, LedgerKind::Policy, "operator", None, None, payload)
    }

    /// The effective configuration a mission started under: its hash and the
    /// layers it was read from (`mission` is its slug, `job` the job it
    /// became when there is one).
    pub fn record_config_loaded(
        &self,
        config: &crate::Provenance,
        mission: Option<&str>,
        job: Option<JobId>,
        now: Millis,
    ) -> AppResult<()> {
        let layers = config
            .layers
            .iter()
            .map(|(tier, source, found)| {
                Payload::new()
                    .str("tier", tier.as_str())
                    .text("source", source)
                    .bool("found", *found)
            })
            .collect();
        let payload = Payload::new()
            .str("event", "config_loaded")
            .opt_str("mission", mission)
            .str("config_sha256", &config.config_sha256)
            .objects("layers", layers);
        self.append(now, LedgerKind::Policy, "swarmux", job, None, payload)
    }

    /// `swarmux config migrate-paths` moved the data of an old install: from
    /// where, to which config and state folders, and what moved.
    pub fn record_paths_migrated(
        &self,
        from: &str,
        config_to: &str,
        state_to: &str,
        moved: &[String],
        now: Millis,
    ) -> AppResult<()> {
        let payload = Payload::new()
            .str("event", "paths_migrated")
            .text("from", from)
            .text("config_to", config_to)
            .text("state_to", state_to)
            .strs("moved", moved);
        self.append(now, LedgerKind::Policy, "operator", None, None, payload)
    }

    /// A hook call that fell back because swarmux could not be reached
    /// (read from the spool at the next start): what the hook printed and
    /// why, and the hash of the setting that decided it.
    pub fn record_fallback(&self, f: &FallbackRecord, now: Millis) -> AppResult<()> {
        let setting = format!("guard.fail={}", f.mode);
        let payload = Payload::new()
            .str("event", "guard_fallback")
            .int("at", f.ts)
            .str("tool", &f.tool)
            .str("input_sha256", &f.input_sha256)
            .str("reason", &f.reason)
            .str("mode", &f.mode)
            .str("line_id", &f.line_id)
            .str("policy_sha256", &hex(&sha256(setting.as_bytes())));
        self.append(now, LedgerKind::Policy, "guard-hook", f.job, None, payload)
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

    fn recent_events(&self, id: JobId, until: Millis, limit: usize) -> AppResult<Vec<StoredEvent>> {
        self.inner.recent_events(id, until, limit)
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

/// A rule store that also records every rule added or removed in the
/// ledger, on the same thread, right after the store changed.
pub struct LedgeredRules {
    inner: Rc<dyn RuleStore>,
    ledger: Rc<LedgerService>,
}

impl LedgeredRules {
    pub fn new(inner: Rc<dyn RuleStore>, ledger: Rc<LedgerService>) -> LedgeredRules {
        LedgeredRules { inner, ledger }
    }

    /// Removes the rules that have expired, recording each as `expired`;
    /// how many there were.
    pub fn prune(&self) -> AppResult<Vec<StoredRule>> {
        let now = self.ledger.now();
        let mut gone = Vec::new();
        for rule in self.inner.list()?.into_iter().filter(|r| r.expired(now)) {
            if self.inner.remove(&rule.id)? {
                self.ledger.record_rule("expired", &rule, now)?;
                gone.push(rule);
            }
        }
        Ok(gone)
    }
}

impl RuleStore for LedgeredRules {
    fn list(&self) -> AppResult<Vec<StoredRule>> {
        self.inner.list()
    }

    fn add(&self, rule: &StoredRule) -> AppResult<()> {
        self.inner.add(rule)?;
        self.ledger.record_rule("added", rule, self.ledger.now())
    }

    fn remove(&self, id: &str) -> AppResult<bool> {
        let was = self.inner.list()?.into_iter().find(|r| r.id == id);
        let removed = self.inner.remove(id)?;
        if let (true, Some(rule)) = (removed, was) {
            self.ledger
                .record_rule("removed", &rule, self.ledger.now())?;
        }
        Ok(removed)
    }
}

/// Puts what the hook processes left in the spool into the ledger, then
/// empties the spool. Returns how many records there were. The ledger is
/// written first: a crash between the two repeats records (each carries
/// its line id) rather than losing them.
pub fn ingest_fallbacks(spool: &dyn FallbackSpool, ledger: &LedgerService) -> AppResult<usize> {
    let records = spool.read()?;
    let now = ledger.now();
    for record in &records {
        ledger.record_fallback(record, now)?;
    }
    if !records.is_empty() {
        spool.clear()?;
    }
    Ok(records.len())
}

#[cfg(test)]
mod tests;
