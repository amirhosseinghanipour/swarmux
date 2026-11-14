//! Use cases of the Guard: deciding hook requests, approvals and the decision log.

mod shared;

use std::collections::VecDeque;
use std::rc::Rc;

use swarmux_domain::alarm::{Challenge, Reversibility, reversibility};
use swarmux_domain::{
    Decision, JobId, LoopTrip, Millis, Pattern, Resolver, RuleScope, RuleSource, StoredRule,
    Verdict, always_rule, may_resolve,
};

use crate::ports::{Clock, DecisionRepository, HookInbox, HookRequest, RuleStore};
use crate::{AppError, AppResult, DecisionExtra, DecisionRecord, Outcome, Source};

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
    /// The knowledge rule that asked; such calls are the owner's alone.
    rule: Option<String>,
    /// Held by a knowledge rule, a protected path or the irreversibility
    /// rule: no "always" is made from it, and no automated decider may
    /// take it from the owner.
    owner_only: bool,
}

/// How a held call ended, for the log.
struct Resolution {
    verdict: Verdict,
    rule: String,
    by: Resolver,
    /// The user's words for the agent, delivered with a denial.
    note: Option<String>,
}

impl Resolution {
    fn by(verdict: Verdict, rule: String, by: Resolver) -> Resolution {
        Resolution {
            verdict,
            rule,
            by,
            note: None,
        }
    }
}

/// Decides tool calls against the policy, holds the ones it can't settle
/// for the user (see [`GuardService::hold`]) and logs every decision.
pub struct GuardService {
    policy: SharedPolicy,
    inbox: Rc<dyn HookInbox>,
    decisions: Rc<dyn DecisionRepository>,
    clock: Rc<dyn Clock>,
    store: Option<Rc<dyn RuleStore>>,
    /// Rules the store would not keep: they last this run, so a reload of
    /// the store must not drop them.
    unsaved: Vec<StoredRule>,
    always: AlwaysSettings,
    last_reload: Option<Millis>,
    timeout_ms: Millis,
    held: Vec<Held>,
    stats: GuardStats,
    recent: VecDeque<DecisionRecord>,
    /// Logged but not yet handed out by `pump`.
    fresh: Vec<DecisionRecord>,
    /// How long instant (code-only) decisions took, in microseconds.
    latencies: VecDeque<u64>,
    /// Where each configured rule came from, for the decision record.
    origins: std::collections::BTreeMap<String, swarmux_domain::layers::Origin>,
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
            unsaved: Vec::new(),
            always: AlwaysSettings::default(),
            last_reload: None,
            timeout_ms: 120_000,
            held: Vec::new(),
            stats: GuardStats::default(),
            recent: VecDeque::new(),
            fresh: Vec::new(),
            latencies: VecDeque::new(),
            origins: Default::default(),
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

    /// The tier and file each configured rule was read from: every decision
    /// record names the one that decided it.
    pub fn with_origins(
        mut self,
        origins: std::collections::BTreeMap<String, swarmux_domain::layers::Origin>,
    ) -> GuardService {
        self.origins = origins;
        self
    }

    /// Where "always allow" applies and for how long.
    pub fn with_always(mut self, always: AlwaysSettings) -> GuardService {
        self.always = always;
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
        let skipped = self.policy.set_learned(kept);
        for rule in &self.unsaved {
            self.policy.add_learned(rule.clone());
        }
        Ok(skipped)
    }

    /// The rules in force, expired ones included.
    pub fn rules(&self) -> Vec<StoredRule> {
        self.policy.learned()
    }

    /// Takes new requests from the inbox, decides them and expires old
    /// approvals.
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
                self.hold(&item.request, decision.rule.as_deref());
            }
            let result = self.log(&item.request, outcome, decision.rule, latency);
            report.errors.extend(result.err());
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
        self.held.len()
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
        if not_saved.is_some() && self.store.is_some() {
            self.unsaved.push(stored.clone());
        }
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
        self.unsaved.retain(|r| r.id != id);
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

    /// The seam for what happens to a call the policy could not settle: a
    /// call no rule matched under `unmatched = "ask"`, or one a rule asked
    /// about. It is held for the owner, who answers in the terminal, and
    /// denied if nobody does within the approval timeout. No model is asked.
    /// (`unmatched = "pass"` never gets here: the policy passes such calls
    /// straight back to Claude Code.)
    ///
    /// A later "decider" extension plugs in here and nowhere else: it would
    /// look at the held call (never one that is `owner_only`) and resolve it
    /// through `resolve`, which refuses any automated allow of an
    /// irreversible call whoever asks (`swarmux_domain::may_resolve`).
    fn hold(&mut self, request: &HookRequest, rule: Option<&str>) {
        // A rule that asks wants the owner and nobody else.
        let owner_only = owner_only(rule);
        let rule = rule
            .filter(|r| swarmux_domain::knowledge::is_rule_reason(r))
            .map(str::to_string);
        self.held.push(Held {
            request: request.clone(),
            asked_at: self.clock.now(),
            rule,
            owner_only,
        });
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
        let latency = waited.saturating_mul(1000);
        let mut record = self.record_for(&held.request, how.verdict.into(), decision.rule, latency);
        record.extra.resolver = Some(how.by.as_str().to_string());
        self.save(record)
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
        let rule_text = rule.clone();
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
            extra: {
                let origin = self.origin_of(rule_text.as_deref());
                DecisionExtra {
                    agent_id: request.agent_id.clone(),
                    agent_type: request.agent_type.clone(),
                    policy_sha256: Some(self.policy.policy_sha256()),
                    rule_tier: origin.as_ref().map(|o| o.tier.as_str().to_string()),
                    rule_source: origin.map(|o| o.source),
                    ..DecisionExtra::default()
                }
            },
        }
    }

    /// The tier and file of the rule `text` names: a stored "always allow"
    /// rule, a configured rule, or a built-in one. Nothing when the text is
    /// a reason no single rule explains.
    fn origin_of(&self, text: Option<&str>) -> Option<swarmux_domain::layers::Origin> {
        use swarmux_domain::layers::{Origin, Tier};
        let text = text?;
        if let Some(r) = self
            .policy
            .learned()
            .into_iter()
            .find(|r| !r.text.is_empty() && text.contains(&r.text))
        {
            return Some(Origin {
                tier: Tier::User,
                source: format!("stored rule {} ({})", r.id, r.scope.as_str()),
            });
        }
        if let Some(o) = self
            .origins
            .iter()
            .filter(|(k, _)| text.contains(k.as_str()))
            .max_by_key(|(k, _)| k.len())
            .map(|(_, o)| o.clone())
        {
            return Some(o);
        }
        swarmux_domain::BUILTIN_DENY
            .iter()
            .chain(swarmux_domain::BUILTIN_ALLOW.iter())
            .any(|b| text.contains(b))
            .then(Origin::core)
    }

    /// Counts, stores and remembers a record.
    fn save(&mut self, record: DecisionRecord) -> AppResult<()> {
        let counter = match record.outcome {
            Outcome::Allow => Some(&mut self.stats.allowed),
            Outcome::Deny => Some(&mut self.stats.denied),
            Outcome::Ask => Some(&mut self.stats.asked),
            Outcome::Pass => Some(&mut self.stats.passed),
            Outcome::Stop => Some(&mut self.stats.stopped),
            Outcome::Route => None,
        };
        if let Some(counter) = counter {
            *counter += 1;
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
