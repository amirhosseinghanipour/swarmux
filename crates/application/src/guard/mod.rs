//! Use cases of the Guard: deciding hook requests, approvals and the decision log.

mod shared;

use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};

use swarmux_domain::{Decision, JobId, LoopTrip, Millis, Pattern, Verdict};

use crate::ports::{
    AdviceAnswer, AdviceReply, Clock, DecisionRepository, HookInbox, HookRequest, RuleWriter,
};
use crate::{AppError, AppResult, DecisionRecord, Jev, Outcome, Source};

pub use shared::SharedPolicy;

/// Decisions kept in memory for the guard panel.
const RECENT: usize = 50;
/// Latencies of instant decisions kept for the p95.
const LATENCIES: usize = 500;

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
    /// Seconds until it is denied for lack of an answer.
    pub secs_left: u64,
}

/// What [`GuardService::always_allow`] did.
#[derive(Clone, Debug, PartialEq)]
pub struct AlwaysAllowed {
    /// The rule added for the rest of this run.
    pub rule: String,
    /// Why the rule wasn't saved to the settings file, when it wasn't.
    pub not_saved: Option<String>,
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
}

/// How a held call ended, for the log.
struct Resolution {
    verdict: Verdict,
    rule: String,
    source: Source,
    confidence: Option<f64>,
    /// `None`: the time the call was held.
    latency_us: Option<u64>,
}

impl Resolution {
    fn by_guard(verdict: Verdict, rule: String) -> Resolution {
        Resolution {
            verdict,
            rule,
            source: Source::Guard,
            confidence: None,
            latency_us: None,
        }
    }
}

type Replies = (Sender<(u64, AdviceReply)>, Receiver<(u64, AdviceReply)>);

/// Decides tool calls against the policy, puts unmatched ones to JEV when it
/// is on, holds the ones that need the user and logs every decision.
pub struct GuardService {
    policy: SharedPolicy,
    inbox: Rc<dyn HookInbox>,
    decisions: Rc<dyn DecisionRepository>,
    clock: Rc<dyn Clock>,
    rules: Option<Rc<dyn RuleWriter>>,
    jev: Option<Rc<Jev>>,
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
            rules: None,
            jev: None,
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

    /// Where "always allow" rules are saved; without one they last this run only.
    pub fn with_rule_writer(mut self, rules: Rc<dyn RuleWriter>) -> GuardService {
        self.rules = Some(rules);
        self
    }

    /// Calls no rule matches go to JEV first while it is on.
    pub fn with_jev(mut self, jev: Rc<Jev>) -> GuardService {
        self.jev = Some(jev);
        self
    }

    /// Takes new requests from the inbox, decides them, applies JEV's answers
    /// and expires old approvals. Never waits for JEV.
    pub fn pump(&mut self) -> GuardReport {
        let mut report = GuardReport::default();
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
                let consulting = self.consult(&item.request);
                self.held.push(Held {
                    request: item.request.clone(),
                    asked_at: self.clock.now(),
                    consulting,
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
            if let Err(e) = self.resolve(id, Resolution::by_guard(Verdict::Deny, rule)) {
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

    pub fn approve(&mut self, request_id: u64) -> AppResult<()> {
        let rule = "approved in swarmux".to_string();
        self.resolve(request_id, Resolution::by_guard(Verdict::Allow, rule))
    }

    pub fn deny(&mut self, request_id: u64) -> AppResult<()> {
        let rule = "denied in swarmux".to_string();
        self.resolve(request_id, Resolution::by_guard(Verdict::Deny, rule))
    }

    /// Approves the call and allows calls exactly like it from now on.
    /// Other held calls the new rule covers are approved too.
    pub fn always_allow(&mut self, request_id: u64) -> AppResult<AlwaysAllowed> {
        let request = self
            .held
            .iter()
            .find(|h| h.request.request_id == request_id)
            .map(|h| h.request.clone())
            .ok_or(AppError::ApprovalNotFound)?;
        let rule = Pattern::exact_for(&request.tool_name, &request.subject, &request.cwd);
        self.adopt(rule, Some(request_id))
    }

    /// Allows the calls `rule` covers from now on, as the user accepted it
    /// (`Bash(cargo test:*)`), and approves the held calls it covers.
    pub fn allow_rule(&mut self, rule: &str) -> AppResult<AlwaysAllowed> {
        let pattern = Pattern::parse(rule)?;
        self.adopt(pattern, None)
    }

    fn adopt(&mut self, rule: Pattern, answering: Option<u64>) -> AppResult<AlwaysAllowed> {
        let text = rule.to_string();
        self.policy.add_allow(rule);
        let not_saved = match &self.rules {
            Some(writer) => writer.add_allow_rule(&text).err().map(|e| e.to_string()),
            None => Some("no settings file to save to".into()),
        };
        if let Some(request_id) = answering {
            self.resolve(
                request_id,
                Resolution::by_guard(Verdict::Allow, text.clone()),
            )?;
        }

        let now_allowed: Vec<(u64, Option<String>)> = self
            .held
            .iter()
            .filter_map(|h| {
                let (d, _) = self.policy.decide(&h.request);
                (d.verdict == Verdict::Allow).then_some((h.request.request_id, d.rule))
            })
            .collect();
        for (id, rule) in now_allowed {
            let rule = rule.unwrap_or_else(|| text.clone());
            self.resolve(id, Resolution::by_guard(Verdict::Allow, rule))?;
        }
        Ok(AlwaysAllowed {
            rule: text,
            not_saved,
        })
    }

    /// Logs that the loop breaker stopped `job`.
    pub fn record_loop(&mut self, job: JobId, trip: &LoopTrip) -> AppResult<()> {
        let request = HookRequest {
            request_id: 0,
            job_id: Some(job),
            tool_name: trip.tool.clone(),
            subject: trip.summary.clone(),
            input: String::new(),
            cwd: String::new(),
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

    /// Puts `request` to JEV if it is on. Returns whether it was.
    fn consult(&self, request: &HookRequest) -> bool {
        let Some(jev) = self.jev.as_ref().filter(|j| j.is_on()) else {
            return false;
        };
        let tx = self.replies.0.clone();
        let id = request.request_id;
        jev.ask_tool(
            request,
            Box::new(move |reply| {
                let _ = tx.send((id, reply));
            }),
        );
        true
    }

    /// A confident answer decides the call; anything else shows it to the
    /// user. Answers for calls decided meanwhile are dropped.
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
        let answer = match &reply.result {
            Ok(advice) => match advice.answer {
                AdviceAnswer::Allow => Ok((Verdict::Allow, advice.confidence)),
                AdviceAnswer::Deny => Ok((Verdict::Deny, advice.confidence)),
                AdviceAnswer::Session(_) => Err("answered another question".to_string()),
            },
            Err(e) => Err(e.to_string()),
        };
        if let Ok((verdict, confidence)) = answer
            && confidence >= threshold
        {
            return self.resolve(
                id,
                Resolution {
                    verdict,
                    rule: format!("jev {confidence:.2}"),
                    source: Source::Jev,
                    confidence: Some(confidence),
                    latency_us: Some(latency_us),
                },
            );
        }
        self.held[i].consulting = false;
        let request = self.held[i].request.clone();
        let (rule, source, confidence) = match answer {
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
        let record = self.record_for(&request, Outcome::Ask, Some(rule), latency_us);
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
        let held = self.held.remove(i);
        let decision = Decision::new(how.verdict, Some(how.rule));
        self.inbox.respond(request_id, &decision);
        let waited = u64::try_from(self.clock.now() - held.asked_at).unwrap_or(0);
        let latency = how.latency_us.unwrap_or(waited.saturating_mul(1000));
        let record = self.record_for(&held.request, how.verdict.into(), decision.rule, latency);
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
