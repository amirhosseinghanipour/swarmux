//! Use cases of the Guard: deciding hook requests, approvals and the decision log.

mod shared;

use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};

use swarmux_domain::alarm::{Challenge, Reversibility, reversibility};
use swarmux_domain::{
    Decision, JobId, LoopTrip, Millis, Pattern, Resolver, RuleScope, RuleSource, StoredRule,
    Verdict, always_rule, may_resolve,
};

use crate::ports::{
    AdviceAnswer, AdviceReply, Clock, DecisionRepository, HookInbox, HookRequest, RuleStore,
};
use crate::{AppError, AppResult, DecisionExtra, DecisionRecord, Jev, JevTrace, Outcome, Source};

use shared::owner_only;
pub use shared::{IRREVERSIBLE_REASON, PROTECTED_REASON, Protection, SharedPolicy, UnattendedJobs};

/// Decisions kept in memory for the guard panel.
const RECENT: usize = 50;
/// Latencies of instant decisions kept for the p95.
const LATENCIES: usize = 500;
/// How often the rules on disk are read again, so a rule removed from
/// another process (`swarmux guard rules remove`) stops applying.
const RELOAD_EVERY_MS: Millis = 2_000;

/// Counts for this run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GuardStats {
    pub allowed: usize,
    pub denied: usize,
    pub asked: usize,
    pub passed: usize,
    pub stopped: usize,
    /// Records JEV made (answers and errors).
    pub jev: usize,
}

impl GuardStats {
    /// Tool-call decisions: allowed, denied, asked and passed.
    pub fn tool_calls(&self) -> usize {
        self.allowed + self.denied + self.asked + self.passed
    }

    /// Every decision counted, loop stops included.
    pub fn total(&self) -> usize {
        self.tool_calls() + self.stopped
    }

    /// `part` as a share of the tool-call decisions; `None` before the first.
    pub fn share(&self, part: usize) -> Option<f64> {
        let all = self.tool_calls();
        (all > 0).then(|| part as f64 / all as f64)
    }
}

/// A held tool call, as the UI shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingApproval {
    pub request: HookRequest,
    /// The knowledge rule that asked, when one did (shown with the call).
    pub rule: Option<String>,
    /// Seconds until it is denied for lack of an answer.
    pub secs_left: u64,
}

/// What [`GuardService::always_allow`] did.
#[derive(Clone, Debug, PartialEq)]
pub struct AlwaysAllowed {
    /// The rule added.
    pub rule: String,
    /// What `swarmux guard rules remove` takes.
    pub id: String,
    pub scope: RuleScope,
    /// Where it applies (the repository or workspace folder).
    pub root: Option<String>,
    /// When it stops, or `None` for never.
    pub expires_at: Option<Millis>,
    /// Why the rule wasn't saved, when it wasn't (then it lasts this run).
    pub not_saved: Option<String>,
}

/// How an irreversible call is confirmed: the person names its target,
/// by typing it or by picking it from the list the prompt showed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Confirmation {
    Typed(String),
    /// The 1-based number of the choice picked.
    Choice(usize),
}

/// What an "always allow" is made with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlwaysSettings {
    pub scope: RuleScope,
    /// `None` is never.
    pub expiry_days: Option<u32>,
}

impl Default for AlwaysSettings {
    fn default() -> AlwaysSettings {
        AlwaysSettings {
            scope: RuleScope::Repo,
            expiry_days: Some(swarmux_domain::DEFAULT_EXPIRY_DAYS),
        }
    }
}

/// What one [`GuardService::pump`] did.
#[derive(Debug, Default)]
pub struct GuardReport {
    pub errors: Vec<AppError>,
    pub changed: bool,
    /// Decisions logged since the last pump (also those made by key presses).
    pub decisions: Vec<DecisionRecord>,
}

struct Held {
    request: HookRequest,
    asked_at: Millis,
    /// JEV is being asked; the user isn't shown the call yet.
    consulting: bool,
    /// The knowledge rule that asked; such calls are the owner's alone.
    rule: Option<String>,
    /// Held by a knowledge rule, a protected path or the irreversibility
    /// rule: no model sees it and no "always" is made from it.
    owner_only: bool,
    /// What JEV was sent, while it is being asked.
    trace: Option<JevTrace>,
}

/// How a held call ended, for the log.
struct Resolution {
    verdict: Verdict,
    rule: String,
    source: Source,
    by: Resolver,
    confidence: Option<f64>,
    /// `None`: the time the call was held.
    latency_us: Option<u64>,
    /// The user's words for the agent, delivered with a denial.
    note: Option<String>,
    jev: Option<JevTrace>,
}

impl Resolution {
    fn by(verdict: Verdict, rule: String, by: Resolver) -> Resolution {
        Resolution {
            verdict,
            rule,
            source: Source::Guard,
            by,
            confidence: None,
            latency_us: None,
            note: None,
            jev: None,
        }
    }
}

type Reply = (u64, AdviceReply);
type Replies = (Sender<Reply>, Receiver<Reply>);

/// Decides tool calls against the policy, puts unmatched ones to JEV when it
/// is on, holds the ones that need the user and logs every decision.
pub struct GuardService {
    policy: SharedPolicy,
    inbox: Rc<dyn HookInbox>,
    decisions: Rc<dyn DecisionRepository>,
    clock: Rc<dyn Clock>,
    store: Option<Rc<dyn RuleStore>>,
    always: AlwaysSettings,
    last_reload: Option<Millis>,
    jev: Option<Rc<Jev>>,
    /// The riskiest call JEV may allow.
    jev_cap: Reversibility,
    replies: Replies,
    timeout_ms: Millis,
    held: Vec<Held>,
    stats: GuardStats,
    recent: VecDeque<DecisionRecord>,
    /// Logged but not yet handed out by `pump`.
    fresh: Vec<DecisionRecord>,
    /// How long instant (code-only) decisions took, in microseconds.
    latencies: VecDeque<u64>,
}

impl GuardService {
    pub fn new(
        policy: SharedPolicy,
        inbox: Rc<dyn HookInbox>,
        decisions: Rc<dyn DecisionRepository>,
        clock: Rc<dyn Clock>,
    ) -> GuardService {
        GuardService {
            policy,
            inbox,
            decisions,
            clock,
            store: None,
            always: AlwaysSettings::default(),
            last_reload: None,
            jev: None,
            jev_cap: Reversibility::R1,
            replies: channel(),
            timeout_ms: 120_000,
            held: Vec::new(),
            stats: GuardStats::default(),
            recent: VecDeque::new(),
            fresh: Vec::new(),
            latencies: VecDeque::new(),
        }
    }

    /// Unanswered approvals are denied after `secs`.
    pub fn with_timeout_secs(mut self, secs: u64) -> GuardService {
        self.timeout_ms = Millis::try_from(secs.saturating_mul(1000)).unwrap_or(Millis::MAX);
        self
    }

    /// Where "always allow" rules are kept; without one they last this run only.
    pub fn with_rule_store(mut self, store: Rc<dyn RuleStore>) -> GuardService {
        self.store = Some(store);
        self
    }

    /// Where "always allow" applies and for how long.
    pub fn with_always(mut self, always: AlwaysSettings) -> GuardService {
        self.always = always;
        self
    }

    /// The riskiest call JEV may allow (it may deny any).
    pub fn with_jev_cap(mut self, cap: Reversibility) -> GuardService {
        self.jev_cap = cap.min(Reversibility::R3);
        self
    }

    /// Calls no rule matches go to JEV first while it is on.
    pub fn with_jev(mut self, jev: Rc<Jev>) -> GuardService {
        self.jev = Some(jev);
        self
    }

    /// Takes the rules kept in the store into the policy. Ones that don't
    /// parse are returned.
    pub fn load_rules(&mut self) -> AppResult<Vec<StoredRule>> {
        let Some(store) = &self.store else {
            return Ok(Vec::new());
        };
        let kept = store.list()?;
        self.last_reload = Some(self.clock.now());
        Ok(self.policy.set_learned(kept))
    }

    /// The rules in force, expired ones included.
    pub fn rules(&self) -> Vec<StoredRule> {
        self.policy.learned()
    }

    /// Takes new requests from the inbox, decides them, applies JEV's answers
    /// and expires old approvals. Never waits for JEV.
    pub fn pump(&mut self) -> GuardReport {
        let mut report = GuardReport::default();
        let due = self
            .last_reload
            .is_none_or(|at| self.clock.now() - at >= RELOAD_EVERY_MS);
        if self.store.is_some() && due {
            report.errors.extend(self.load_rules().err());
        }
        for item in self.inbox.poll() {
            report.changed = true;
            let (decision, latency) = match item.answered {
                Some(answered) => {
                    self.latencies.push_back(answered.1);
                    answered
                }
                None => {
                    let (decision, latency) = self.policy.decide(&item.request);
                    self.latencies.push_back(latency);
                    if decision.verdict != Verdict::Ask {
                        self.inbox.respond(item.request.request_id, &decision);
                    }
                    (decision, latency)
                }
            };
            let outcome = Outcome::from(decision.verdict);
            if decision.verdict == Verdict::Ask {
                // A rule that asks wants the owner, not a model.
                let owner = owner_only(decision.rule.as_deref());
                let rule = decision
                    .rule
                    .clone()
                    .filter(|r| swarmux_domain::knowledge::is_rule_reason(r));
                let trace = if owner {
                    None
                } else {
                    self.consult(&item.request)
                };
                self.held.push(Held {
                    request: item.request.clone(),
                    asked_at: self.clock.now(),
                    consulting: trace.is_some(),
                    rule,
                    owner_only: owner,
                    trace,
                });
            }
            let result = self.log(&item.request, outcome, decision.rule, latency);
            report.errors.extend(result.err());
        }
        while let Ok((id, reply)) = self.replies.1.try_recv() {
            report.changed = true;
            if let Err(e) = self.apply_advice(id, reply) {
                report.errors.push(e);
            }
        }
        let now = self.clock.now();
        let expired: Vec<u64> = self
            .held
            .iter()
            .filter(|h| now - h.asked_at >= self.timeout_ms)
            .map(|h| h.request.request_id)
            .collect();
        for id in expired {
            report.changed = true;
            let rule = format!("no answer within {} s", self.timeout_ms / 1000);
            let how = Resolution::by(Verdict::Deny, rule, Resolver::Timeout);
            if let Err(e) = self.resolve(id, how) {
                report.errors.push(e);
            }
        }
        while self.latencies.len() > LATENCIES {
            self.latencies.pop_front();
        }
        report.decisions = std::mem::take(&mut self.fresh);
        report.changed |= !report.decisions.is_empty();
        report
    }

    /// 95th percentile of how long the policy took to decide, in
    /// microseconds; `None` before the first decision.
    pub fn p95_latency_us(&self) -> Option<u64> {
        let mut all: Vec<u64> = self.latencies.iter().copied().collect();
        if all.is_empty() {
            return None;
        }
        all.sort_unstable();
        let rank = (all.len() * 95).div_ceil(100).max(1) - 1;
        Some(all[rank])
    }

    /// Held calls waiting for the user, oldest first.
    pub fn approvals(&self) -> Vec<PendingApproval> {
        let now = self.clock.now();
        self.held
            .iter()
            .filter(|h| !h.consulting)
            .map(|h| PendingApproval {
                request: h.request.clone(),
                rule: h.rule.clone(),
                secs_left: u64::try_from((h.asked_at + self.timeout_ms - now).max(0) / 1000)
                    .unwrap_or(0),
            })
            .collect()
    }

    /// Calls waiting for the user.
    pub fn pending_count(&self) -> usize {
        self.held.iter().filter(|h| !h.consulting).count()
    }

    /// Calls JEV is deciding right now.
    pub fn consulting_count(&self) -> usize {
        self.held.iter().filter(|h| h.consulting).count()
    }

    /// JEV, when it is configured.
    pub fn jev(&self) -> Option<&Rc<Jev>> {
        self.jev.as_ref()
    }

    /// Allows a held call. An irreversible one is refused: only
    /// [`confirm_irreversible`](Self::confirm_irreversible) allows it.
    pub fn approve(&mut self, request_id: u64) -> AppResult<()> {
        let rule = "approved in swarmux".to_string();
        let how = Resolution::by(Verdict::Allow, rule, Resolver::HumanKey);
        self.resolve(request_id, how)
    }

    pub fn deny(&mut self, request_id: u64) -> AppResult<()> {
        let rule = "denied in swarmux".to_string();
        let how = Resolution::by(Verdict::Deny, rule, Resolver::HumanKey);
        self.resolve(request_id, how)
    }

    /// Allows a held irreversible call after its target was named. The
    /// check is here, not in a client: `challenge` is rebuilt from the held
    /// call and must be the same, and `answer` must name the target after
    /// the prompt armed. Any other way to allow such a call is refused.
    pub fn confirm_irreversible(
        &mut self,
        challenge: &Challenge,
        answer: &Confirmation,
    ) -> AppResult<()> {
        let id = challenge.request;
        let request = self
            .held
            .iter()
            .find(|h| h.request.request_id == id)
            .map(|h| h.request.clone())
            .ok_or(AppError::ApprovalNotFound)?;
        if reversibility(&request.tool_name, &request.subject, &request.cwd) != Reversibility::R4 {
            return Err(AppError::Guard(
                "that call doesn't need a typed confirmation".into(),
            ));
        }
        let expected = Challenge::new(
            id,
            &request.tool_name,
            &request.subject,
            challenge.opened_at(),
        );
        if *challenge != expected {
            return Err(AppError::Guard(
                "that confirmation is for another call".into(),
            ));
        }
        let now = self.clock.now();
        let named = match answer {
            Confirmation::Typed(text) => expected.accepts_text(text, now),
            Confirmation::Choice(n) => expected.accepts_choice(*n, now),
        };
        if !named {
            return Err(AppError::Guard(
                "that is not the target, or the prompt hasn't armed yet".into(),
            ));
        }
        let rule = format!("confirmed by naming {}", expected.target);
        let how = Resolution::by(Verdict::Allow, rule, Resolver::HumanTyped);
        self.resolve(id, how)
    }

    /// Denies the held call and tells the agent why: `note` is delivered with
    /// the denial, as part of the reason Claude Code shows the agent. The
    /// decision log records the denial as any other. A blank note is a plain
    /// denial.
    pub fn deny_with_note(&mut self, request_id: u64, note: &str) -> AppResult<()> {
        let rule = "denied in swarmux, with a note".to_string();
        let how = Resolution {
            note: Some(note.trim().to_string()).filter(|n| !n.is_empty()),
            ..Resolution::by(Verdict::Deny, rule, Resolver::HumanKey)
        };
        self.resolve(request_id, how)
    }

    /// Approves the call and allows calls exactly like it from now on, in
    /// the repository it was made in and until the rule expires. Refused for
    /// irreversible calls, compound shell lines and calls whose rule would
    /// be the bare tool name; those can be allowed once. Other held calls the
    /// new rule covers are approved too.
    pub fn always_allow(&mut self, request_id: u64) -> AppResult<AlwaysAllowed> {
        let held = self
            .held
            .iter()
            .find(|h| h.request.request_id == request_id)
            .ok_or(AppError::ApprovalNotFound)?;
        let request = held.request.clone();
        let owner_only = held.owner_only;
        let rule = always_rule(&request.tool_name, &request.subject, &request.cwd)
            .map_err(|r| AppError::Guard(r.to_string()))?;
        if owner_only {
            return Err(AppError::Guard(
                "this call is held by a protection; it can only be allowed once".into(),
            ));
        }
        self.adopt(rule, RuleSource::AlwaysAllow, &request, Some(request_id))
    }

    /// Allows the calls `rule` covers from now on, as the user accepted it
    /// (`Bash(cargo test:*)`), in the repository of the held call `from`
    /// (the one that earned the offer), and approves the held calls it
    /// covers.
    pub fn allow_rule(&mut self, rule: &str, from: u64) -> AppResult<AlwaysAllowed> {
        let pattern = Pattern::parse(rule)?;
        if pattern.is_bare() {
            return Err(AppError::Guard(format!(
                "that rule would allow every use of {}; write the rule you want in config.toml",
                pattern.tool()
            )));
        }
        let request = self
            .held
            .iter()
            .find(|h| h.request.request_id == from)
            .map(|h| h.request.clone())
            .ok_or(AppError::ApprovalNotFound)?;
        self.adopt(pattern, RuleSource::Suggestion, &request, None)
    }

    fn adopt(
        &mut self,
        rule: Pattern,
        source: RuleSource,
        request: &HookRequest,
        answering: Option<u64>,
    ) -> AppResult<AlwaysAllowed> {
        let text = rule.to_string();
        let now = self.clock.now();
        let (scope, root) = self.scope_for(request)?;
        let stored = StoredRule::new(
            &text,
            scope,
            root.as_deref(),
            source,
            now,
            StoredRule::expiry_after(now, self.always.expiry_days),
        );
        self.policy.add_learned(stored.clone());
        let not_saved = match &self.store {
            Some(store) => store.add(&stored).err().map(|e| e.to_string()),
            None => Some("no store to keep it in".into()),
        };
        if let Some(request_id) = answering {
            let how = Resolution::by(Verdict::Allow, text.clone(), Resolver::HumanKey);
            self.resolve(request_id, how)?;
        }
        let now_allowed: Vec<(u64, Option<String>)> = self
            .held
            .iter()
            .filter(|h| !h.owner_only)
            .filter_map(|h| {
                let (d, _) = self.policy.decide(&h.request);
                (d.verdict == Verdict::Allow).then_some((h.request.request_id, d.rule))
            })
            .collect();
        for (id, rule) in now_allowed {
            let rule = rule.unwrap_or_else(|| text.clone());
            self.resolve(id, Resolution::by(Verdict::Allow, rule, Resolver::Rule))?;
        }
        Ok(AlwaysAllowed {
            rule: text,
            id: stored.id,
            scope,
            root,
            expires_at: stored.expires_at,
            not_saved,
        })
    }

    /// Where a rule made from `request` applies, as the settings say.
    fn scope_for(&self, request: &HookRequest) -> AppResult<(RuleScope, Option<String>)> {
        let roots = self.policy.roots();
        let repo = roots
            .and_then(|r| r.repo_root(request.job_id, &request.cwd))
            .or_else(|| (!request.cwd.is_empty()).then(|| request.cwd.clone()));
        match self.always.scope {
            RuleScope::Global => Ok((RuleScope::Global, None)),
            RuleScope::Repo => match repo {
                Some(root) => Ok((RuleScope::Repo, Some(root))),
                None => Err(AppError::Guard(
                    "can't tell which repository this call is in, so a rule for it has no home"
                        .into(),
                )),
            },
            RuleScope::Workspace => {
                match roots.and_then(|r| r.workspace_root(request.job_id, &request.cwd)) {
                    Some(root) => Ok((RuleScope::Workspace, Some(root))),
                    // A job of a plain repository has no workspace: its
                    // repository is the narrowest home there is.
                    None => repo
                        .map(|root| (RuleScope::Repo, Some(root)))
                        .ok_or_else(|| {
                            AppError::Guard(
                                "no repository or workspace to make the rule for".into(),
                            )
                        }),
                }
            }
        }
    }

    /// Removes the rule called `id` from the policy and the store; whether
    /// there was one.
    pub fn remove_rule(&mut self, id: &str) -> AppResult<bool> {
        let was = self.policy.remove_learned(id);
        let stored = match &self.store {
            Some(store) => store.remove(id)?,
            None => false,
        };
        Ok(was || stored)
    }

    /// Logs that the loop breaker stopped `job`.
    pub fn record_loop(&mut self, job: JobId, trip: &LoopTrip) -> AppResult<()> {
        let request = HookRequest {
            request_id: 0,
            job_id: Some(job),
            tool_name: trip.tool.clone(),
            subject: trip.summary.clone(),
            ..HookRequest::default()
        };
        let record = self.record_for(&request, Outcome::Stop, Some(trip.reason()), 0);
        self.save(DecisionRecord {
            source: Source::Loop,
            ..record
        })
    }

    pub fn stats(&self) -> GuardStats {
        self.stats
    }

    /// Decisions of this run, newest first.
    pub fn recent(&self) -> impl Iterator<Item = &DecisionRecord> {
        self.recent.iter()
    }

    /// The current policy (configured, built-in and added rules).
    pub fn policy(&self) -> &SharedPolicy {
        &self.policy
    }

    /// Puts `request` to JEV if it is on and the call may be put to it:
    /// never an irreversible one (only a person confirms those), and never
    /// one with nothing to judge (a tool whose subject is unknown). What
    /// was sent is returned.
    fn consult(&self, request: &HookRequest) -> Option<JevTrace> {
        let jev = self.jev.as_ref().filter(|j| j.is_on())?;
        if reversibility(&request.tool_name, &request.subject, &request.cwd) == Reversibility::R4 {
            return None;
        }
        let tx = self.replies.0.clone();
        let id = request.request_id;
        jev.ask_tool(
            request,
            Box::new(move |reply| {
                let _ = tx.send((id, reply));
            }),
        )
    }

    /// A confident answer decides the call; anything else shows it to the
    /// user. A confident allow decides only calls up to the configured risk;
    /// above it the call goes to the user, whatever JEV said. Answers for
    /// calls decided meanwhile are dropped.
    fn apply_advice(&mut self, id: u64, reply: AdviceReply) -> AppResult<()> {
        let Some(i) = self
            .held
            .iter()
            .position(|h| h.request.request_id == id && h.consulting)
        else {
            return Ok(());
        };
        let threshold = self.jev.as_ref().map_or(1.0, |j| j.threshold());
        let latency_us = reply.latency_ms.saturating_mul(1000);
        let trace = self.held[i].trace.clone();
        let request = self.held[i].request.clone();
        let risk = reversibility(&request.tool_name, &request.subject, &request.cwd);
        let answer = match &reply.result {
            Ok(advice) => match advice.answer {
                AdviceAnswer::Allow => Ok((Verdict::Allow, advice.confidence)),
                AdviceAnswer::Deny => Ok((Verdict::Deny, advice.confidence)),
                _ => Err("answered another question".to_string()),
            },
            Err(e) => Err(e.to_string()),
        };
        let too_risky = |v: Verdict| v == Verdict::Allow && risk > self.jev_cap;
        if let Ok((verdict, confidence)) = answer
            && confidence >= threshold
            && !too_risky(verdict)
        {
            return self.resolve(
                id,
                Resolution {
                    verdict,
                    rule: format!("jev {confidence:.2}"),
                    source: Source::Jev,
                    by: Resolver::Model,
                    confidence: Some(confidence),
                    latency_us: Some(latency_us),
                    note: None,
                    jev: trace,
                },
            );
        }
        self.held[i].consulting = false;
        let (rule, source, confidence) = match answer {
            Ok((Verdict::Allow, confidence)) if confidence >= threshold => (
                format!(
                    "jev: allow {confidence:.2}, but {} is above what it may allow ({}), asking you",
                    risk.as_str(),
                    self.jev_cap.as_str()
                ),
                Source::Jev,
                Some(confidence),
            ),
            Ok((verdict, confidence)) => (
                format!(
                    "jev: {} {confidence:.2} < {threshold:.2}, asking you",
                    verdict.as_str()
                ),
                Source::Jev,
                Some(confidence),
            ),
            Err(why) => (format!("jev: {why}; asking you"), Source::JevError, None),
        };
        let mut record = self.record_for(&request, Outcome::Ask, Some(rule), latency_us);
        record.extra.jev = trace;
        self.save(DecisionRecord {
            source,
            confidence,
            ..record
        })
    }

    fn resolve(&mut self, request_id: u64, how: Resolution) -> AppResult<()> {
        let i = self
            .held
            .iter()
            .position(|h| h.request.request_id == request_id)
            .ok_or(AppError::ApprovalNotFound)?;
        // The one rule about irreversible calls, whoever asks: see
        // `swarmux_domain::may_resolve`.
        let r = {
            let q = &self.held[i].request;
            reversibility(&q.tool_name, &q.subject, &q.cwd)
        };
        if !may_resolve(r, how.verdict, how.by) {
            return Err(AppError::NeedsTypedConfirmation);
        }
        let held = self.held.remove(i);
        let mut decision = Decision::new(how.verdict, Some(how.rule));
        if let Some(note) = &how.note {
            decision = decision.with_note(note);
        }
        self.inbox.respond(request_id, &decision);
        let waited = u64::try_from(self.clock.now() - held.asked_at).unwrap_or(0);
        let latency = how.latency_us.unwrap_or(waited.saturating_mul(1000));
        let mut record = self.record_for(&held.request, how.verdict.into(), decision.rule, latency);
        record.extra.resolver = Some(how.by.as_str().to_string());
        record.extra.jev = how.jev;
        self.save(DecisionRecord {
            source: how.source,
            confidence: how.confidence,
            ..record
        })
    }

    fn log(
        &mut self,
        request: &HookRequest,
        outcome: Outcome,
        rule: Option<String>,
        latency_us: u64,
    ) -> AppResult<()> {
        let record = self.record_for(request, outcome, rule, latency_us);
        self.save(record)
    }

    fn record_for(
        &self,
        request: &HookRequest,
        outcome: Outcome,
        rule: Option<String>,
        latency_us: u64,
    ) -> DecisionRecord {
        DecisionRecord {
            job_id: request.job_id,
            tool: request.tool_name.clone(),
            subject: request.subject.clone(),
            outcome,
            rule,
            latency_us,
            ts: self.clock.now(),
            source: Source::Guard,
            confidence: None,
            extra: DecisionExtra {
                agent_id: request.agent_id.clone(),
                agent_type: request.agent_type.clone(),
                ..DecisionExtra::default()
            },
        }
    }

    /// Counts, stores and remembers a record.
    fn save(&mut self, record: DecisionRecord) -> AppResult<()> {
        let by_jev = matches!(record.source, Source::Jev | Source::JevError);
        let counter = match record.outcome {
            Outcome::Allow => Some(&mut self.stats.allowed),
            Outcome::Deny => Some(&mut self.stats.denied),
            // JEV handing a held call to the user isn't a second ask.
            Outcome::Ask if by_jev => None,
            Outcome::Ask => Some(&mut self.stats.asked),
            Outcome::Pass => Some(&mut self.stats.passed),
            Outcome::Stop => Some(&mut self.stats.stopped),
            Outcome::Route => None,
        };
        if let Some(counter) = counter {
            *counter += 1;
        }
        if by_jev {
            self.stats.jev += 1;
        }
        let saved = self.decisions.record(&record);
        self.fresh.push(record.clone());
        self.recent.push_front(record);
        self.recent.truncate(RECENT);
        saved
    }
}

#[cfg(test)]
mod tests;
