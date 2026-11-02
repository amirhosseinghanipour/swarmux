//! Use cases of the Guard: deciding hook requests, approvals and the decision log.

mod shared;

use std::collections::VecDeque;
use std::rc::Rc;

use swarmux_domain::{Decision, JobId, LoopTrip, Millis, Pattern, Verdict};

use crate::ports::{Clock, DecisionRepository, HookInbox, HookRequest, RuleWriter};
use crate::{AppError, AppResult};

pub use shared::SharedPolicy;

/// Decisions kept in memory for the guard panel.
const RECENT: usize = 50;

/// How a decision ended, as logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Allow,
    Deny,
    /// Held for the user; a later record says how it ended.
    Ask,
    Pass,
    /// The loop breaker stopped the job.
    Stop,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Allow => "allow",
            Outcome::Deny => "deny",
            Outcome::Ask => "ask",
            Outcome::Pass => "pass",
            Outcome::Stop => "stop",
        }
    }

    pub fn parse(s: &str) -> Option<Outcome> {
        Some(match s {
            "allow" => Outcome::Allow,
            "deny" => Outcome::Deny,
            "ask" => Outcome::Ask,
            "pass" => Outcome::Pass,
            "stop" => Outcome::Stop,
            _ => return None,
        })
    }
}

impl From<Verdict> for Outcome {
    fn from(v: Verdict) -> Outcome {
        match v {
            Verdict::Allow => Outcome::Allow,
            Verdict::Deny => Outcome::Deny,
            Verdict::Ask => Outcome::Ask,
            Verdict::Pass => Outcome::Pass,
        }
    }
}

/// One line of the decision log.
#[derive(Clone, Debug, PartialEq)]
pub struct DecisionRecord {
    pub job_id: Option<JobId>,
    pub tool: String,
    pub subject: String,
    pub outcome: Outcome,
    pub rule: Option<String>,
    pub latency_us: u64,
    pub ts: Millis,
}

/// Counts for this run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GuardStats {
    pub allowed: usize,
    pub denied: usize,
    pub asked: usize,
    pub passed: usize,
    pub stopped: usize,
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
}

struct Held {
    request: HookRequest,
    asked_at: Millis,
}

/// Decides tool calls against the policy, holds the ones that need the user
/// and logs every decision.
pub struct GuardService {
    policy: SharedPolicy,
    inbox: Rc<dyn HookInbox>,
    decisions: Rc<dyn DecisionRepository>,
    clock: Rc<dyn Clock>,
    rules: Option<Rc<dyn RuleWriter>>,
    timeout_ms: Millis,
    held: Vec<Held>,
    stats: GuardStats,
    recent: VecDeque<DecisionRecord>,
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
            timeout_ms: 120_000,
            held: Vec::new(),
            stats: GuardStats::default(),
            recent: VecDeque::new(),
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

    /// Takes new requests from the inbox, decides them and expires old approvals.
    pub fn pump(&mut self) -> GuardReport {
        let mut report = GuardReport::default();
        for item in self.inbox.poll() {
            report.changed = true;
            let (decision, latency) = match item.answered {
                Some(answered) => answered,
                None => {
                    let (decision, latency) = self.policy.decide(&item.request);
                    if decision.verdict != Verdict::Ask {
                        self.inbox.respond(item.request.request_id, &decision);
                    }
                    (decision, latency)
                }
            };
            let outcome = Outcome::from(decision.verdict);
            if decision.verdict == Verdict::Ask {
                self.held.push(Held {
                    request: item.request.clone(),
                    asked_at: self.clock.now(),
                });
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
            if let Err(e) = self.resolve(id, Verdict::Deny, rule) {
                report.errors.push(e);
            }
        }
        report
    }

    /// Held calls, oldest first.
    pub fn approvals(&self) -> Vec<PendingApproval> {
        let now = self.clock.now();
        self.held
            .iter()
            .map(|h| PendingApproval {
                request: h.request.clone(),
                secs_left: u64::try_from((h.asked_at + self.timeout_ms - now).max(0) / 1000)
                    .unwrap_or(0),
            })
            .collect()
    }

    pub fn pending_count(&self) -> usize {
        self.held.len()
    }

    pub fn approve(&mut self, request_id: u64) -> AppResult<()> {
        self.resolve(request_id, Verdict::Allow, "approved in swarmux".into())
    }

    pub fn deny(&mut self, request_id: u64) -> AppResult<()> {
        self.resolve(request_id, Verdict::Deny, "denied in swarmux".into())
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
        let text = rule.to_string();
        self.policy.add_allow(rule);
        let not_saved = match &self.rules {
            Some(writer) => writer.add_allow_rule(&text).err().map(|e| e.to_string()),
            None => Some("no settings file to save to".into()),
        };
        self.resolve(request_id, Verdict::Allow, text.clone())?;

        let now_allowed: Vec<(u64, Option<String>)> = self
            .held
            .iter()
            .filter_map(|h| {
                let (d, _) = self.policy.decide(&h.request);
                (d.verdict == Verdict::Allow).then_some((h.request.request_id, d.rule))
            })
            .collect();
        for (id, rule) in now_allowed {
            self.resolve(id, Verdict::Allow, rule.unwrap_or_else(|| text.clone()))?;
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
        self.log(&request, Outcome::Stop, Some(trip.reason()), 0)
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

    fn resolve(&mut self, request_id: u64, verdict: Verdict, rule: String) -> AppResult<()> {
        let i = self
            .held
            .iter()
            .position(|h| h.request.request_id == request_id)
            .ok_or(AppError::ApprovalNotFound)?;
        let held = self.held.remove(i);
        let decision = Decision::new(verdict, Some(rule));
        self.inbox.respond(request_id, &decision);
        let waited = u64::try_from(self.clock.now() - held.asked_at).unwrap_or(0);
        self.log(
            &held.request,
            verdict.into(),
            decision.rule,
            waited.saturating_mul(1000),
        )
    }

    fn log(
        &mut self,
        request: &HookRequest,
        outcome: Outcome,
        rule: Option<String>,
        latency_us: u64,
    ) -> AppResult<()> {
        let counter = match outcome {
            Outcome::Allow => &mut self.stats.allowed,
            Outcome::Deny => &mut self.stats.denied,
            Outcome::Ask => &mut self.stats.asked,
            Outcome::Pass => &mut self.stats.passed,
            Outcome::Stop => &mut self.stats.stopped,
        };
        *counter += 1;
        let record = DecisionRecord {
            job_id: request.job_id,
            tool: request.tool_name.clone(),
            subject: request.subject.clone(),
            outcome,
            rule,
            latency_us,
            ts: self.clock.now(),
        };
        let saved = self.decisions.record(&record);
        self.recent.push_front(record);
        self.recent.truncate(RECENT);
        saved
    }
}

#[cfg(test)]
mod tests;
