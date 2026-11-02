//! "Let swarmux route": picks the session a prompt should go to, with JEV
//! when it is on and confident, else one Haiku call, else the selected session.

use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};

use swarmux_domain::{Millis, Session, SessionId};

use crate::jev::{SUMMARY_MAX, summarize};
use crate::ports::{
    AdviceAnswer, AdviceReply, Clock, DecisionRepository, JobRepository, PromptRouter,
    RouteCandidate, RouteReply, RouteRequest, SessionRepository,
};
use crate::{AppError, AppResult, DecisionRecord, Jev, Outcome, Source};

/// How much of the prompt the router reads.
pub const ROUTE_PROMPT_MAX: usize = 2000;
/// Recent prompts shown to the router per session.
const RECENT_PROMPTS: usize = 3;
const PREVIEW_MAX: usize = 100;
/// Jobs looked at for those previews.
const HISTORY_SCAN: usize = 200;
/// Routing decisions kept in memory for the dashboard.
const RECENT: usize = 20;

/// Where a routed prompt goes, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Routed {
    /// The ticket [`RoutingService::start`] returned.
    pub ticket: u64,
    pub session: Session,
    /// The model the router suggests (shown, not applied).
    pub model_hint: Option<String>,
    pub reason: String,
    pub source: Source,
    pub confidence: Option<f64>,
    /// What asking the router cost.
    pub cost_usd: f64,
}

impl Routed {
    /// One line for the status bar and the job log.
    pub fn summary(&self) -> String {
        let by = match (self.source, self.confidence) {
            (Source::Jev, Some(c)) => format!("jev {c:.2}"),
            (source, _) => source.as_str().to_string(),
        };
        let mut notes = vec![by];
        notes.extend(self.notes());
        format!(
            "routed → {} ({}): {}",
            self.session.name,
            notes.join(", "),
            self.reason
        )
    }

    /// The decision log's rule: `→ name (notes): reason`.
    pub fn rule(&self) -> String {
        let notes = self.notes();
        let notes = if notes.is_empty() {
            String::new()
        } else {
            format!(" ({})", notes.join(", "))
        };
        format!("→ {}{notes}: {}", self.session.name, self.reason)
    }

    fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        if let Some(m) = &self.model_hint {
            notes.push(format!("suggests {m}"));
        }
        if self.cost_usd > 0.0 {
            notes.push(format!("${:.4}", self.cost_usd));
        }
        notes
    }
}

/// What one [`RoutingService::pump`] did.
#[derive(Debug, Default)]
pub struct RouteReport {
    pub routed: Vec<Routed>,
    pub errors: Vec<AppError>,
}

enum Msg {
    Jev(AdviceReply),
    Router(RouteReply),
}

struct Pending {
    ticket: u64,
    prompt: String,
    candidates: Vec<Session>,
    fallback: Session,
    started: Millis,
}

/// Routes prompts without ever blocking the caller: answers arrive in
/// [`RoutingService::pump`].
pub struct RoutingService {
    sessions: Rc<dyn SessionRepository>,
    jobs: Rc<dyn JobRepository>,
    decisions: Rc<dyn DecisionRepository>,
    clock: Rc<dyn Clock>,
    router: Option<Rc<dyn PromptRouter>>,
    jev: Option<Rc<Jev>>,
    tx: Sender<(u64, Msg)>,
    rx: Receiver<(u64, Msg)>,
    pending: Vec<Pending>,
    /// Routed prompts not yet reported, with the prompt and start time.
    ready: Vec<(Routed, String, Millis)>,
    next: u64,
    recent: VecDeque<DecisionRecord>,
}

impl RoutingService {
    pub fn new(
        sessions: Rc<dyn SessionRepository>,
        jobs: Rc<dyn JobRepository>,
        decisions: Rc<dyn DecisionRepository>,
        clock: Rc<dyn Clock>,
    ) -> RoutingService {
        let (tx, rx) = channel();
        RoutingService {
            sessions,
            jobs,
            decisions,
            clock,
            router: None,
            jev: None,
            tx,
            rx,
            pending: Vec::new(),
            ready: Vec::new(),
            next: 0,
            recent: VecDeque::new(),
        }
    }

    pub fn with_router(mut self, router: Rc<dyn PromptRouter>) -> RoutingService {
        self.router = Some(router);
        self
    }

    /// JEV is asked first while it is on.
    pub fn with_jev(mut self, jev: Rc<Jev>) -> RoutingService {
        self.jev = Some(jev);
        self
    }

    /// Starts routing `prompt`; `fallback` (the selected session) is used
    /// when no model can decide. Returns a ticket matched by [`Routed::ticket`].
    pub fn start(&mut self, prompt: &str, fallback: SessionId) -> AppResult<u64> {
        let candidates = self.sessions.list_active()?;
        let selected = candidates
            .iter()
            .find(|s| s.id == fallback)
            .or(candidates.first())
            .cloned()
            .ok_or(AppError::SessionNotFound)?;
        self.next += 1;
        let pending = Pending {
            ticket: self.next,
            prompt: summarize(prompt, ROUTE_PROMPT_MAX),
            candidates,
            fallback: selected,
            started: self.clock.now(),
        };
        if pending.candidates.len() == 1 {
            let routed = fallback_route(&pending, Source::Fallback, "the only session".into());
            self.finish(&pending, routed);
        } else if let Some(jev) = self.jev.as_ref().filter(|j| j.is_on()) {
            let names = pending.candidates.iter().map(|s| s.name.clone()).collect();
            jev.ask_session(&pending.prompt, names, self.sink(pending.ticket, Msg::Jev));
            self.pending.push(pending);
        } else {
            self.ask_router(pending);
        }
        Ok(self.next)
    }

    /// Prompts being routed.
    pub fn in_flight(&self) -> usize {
        self.pending.len()
    }

    /// Routing decisions of this run, newest first.
    pub fn recent(&self) -> impl Iterator<Item = &DecisionRecord> {
        self.recent.iter()
    }

    /// Applies answers that arrived and returns the prompts routed since the
    /// last call.
    pub fn pump(&mut self) -> RouteReport {
        let mut report = RouteReport::default();
        while let Ok((ticket, msg)) = self.rx.try_recv() {
            let Some(i) = self.pending.iter().position(|p| p.ticket == ticket) else {
                continue;
            };
            let pending = self.pending.remove(i);
            match msg {
                Msg::Jev(reply) => self.on_jev(pending, reply, &mut report),
                Msg::Router(reply) => {
                    let routed = on_router(&pending, reply);
                    self.finish(&pending, routed);
                }
            }
        }
        for (routed, prompt, started) in std::mem::take(&mut self.ready) {
            let latency = u64::try_from(self.clock.now() - started).unwrap_or(0);
            let record = DecisionRecord {
                source: routed.source,
                confidence: routed.confidence,
                latency_us: latency.saturating_mul(1000),
                ..self.record(&prompt, routed.rule())
            };
            report.errors.extend(self.save(record).err());
            report.routed.push(routed);
        }
        report
    }

    fn on_jev(&mut self, pending: Pending, reply: AdviceReply, report: &mut RouteReport) {
        let threshold = self.jev.as_ref().map_or(1.0, |j| j.threshold());
        let latency_us = reply.latency_ms.saturating_mul(1000);
        let picked = match reply.result {
            Ok(advice) => match advice.answer {
                AdviceAnswer::Session(name) => match find(&pending.candidates, &name) {
                    Some(session) => Ok((session.clone(), advice.confidence)),
                    None => Err(format!("unknown session {name:?}")),
                },
                _ => Err("answered another question".into()),
            },
            Err(e) => Err(e.to_string()),
        };
        let (rule, source, confidence) = match picked {
            Ok((session, confidence)) if confidence >= threshold => {
                let routed = Routed {
                    ticket: pending.ticket,
                    session,
                    model_hint: None,
                    reason: "jev's pick".into(),
                    source: Source::Jev,
                    confidence: Some(confidence),
                    cost_usd: 0.0,
                };
                self.finish(&pending, routed);
                return;
            }
            Ok((session, confidence)) => (
                format!(
                    "→ {}? {confidence:.2} < {threshold:.2}, asking haiku",
                    session.name
                ),
                Source::Jev,
                Some(confidence),
            ),
            Err(why) => (format!("jev: {why}"), Source::JevError, None),
        };
        let record = DecisionRecord {
            source,
            confidence,
            latency_us,
            ..self.record(&pending.prompt, rule)
        };
        report.errors.extend(self.save(record).err());
        self.ask_router(pending);
    }

    fn ask_router(&mut self, pending: Pending) {
        let Some(router) = self.router.clone() else {
            let routed = fallback_route(&pending, Source::Fallback, "no router".into());
            self.finish(&pending, routed);
            return;
        };
        let request = RouteRequest {
            prompt: pending.prompt.clone(),
            candidates: self.candidates(&pending.candidates),
        };
        router.route(request, self.sink(pending.ticket, Msg::Router));
        self.pending.push(pending);
    }

    fn sink<T: Send + 'static>(
        &self,
        ticket: u64,
        wrap: fn(T) -> Msg,
    ) -> Box<dyn FnOnce(T) + Send> {
        let tx = self.tx.clone();
        Box::new(move |reply| {
            let _ = tx.send((ticket, wrap(reply)));
        })
    }

    /// Sessions as the router sees them, with their latest prompts.
    fn candidates(&self, sessions: &[Session]) -> Vec<RouteCandidate> {
        let history = self.jobs.summaries(HISTORY_SCAN).unwrap_or_default();
        sessions
            .iter()
            .map(|s| RouteCandidate {
                name: s.name.clone(),
                cwd: s.cwd.display().to_string(),
                model: s.model.clone(),
                recent: history
                    .iter()
                    .filter(|j| j.session_id == s.id && j.review_of.is_none())
                    .take(RECENT_PROMPTS)
                    .map(|j| summarize(&j.prompt, PREVIEW_MAX))
                    .collect(),
            })
            .collect()
    }

    fn finish(&mut self, pending: &Pending, routed: Routed) {
        self.ready
            .push((routed, pending.prompt.clone(), pending.started));
    }

    fn record(&self, prompt: &str, rule: String) -> DecisionRecord {
        DecisionRecord {
            job_id: None,
            tool: "route".into(),
            subject: summarize(prompt, SUMMARY_MAX),
            outcome: Outcome::Route,
            rule: Some(rule),
            latency_us: 0,
            ts: self.clock.now(),
            source: Source::Guard,
            confidence: None,
        }
    }

    fn save(&mut self, record: DecisionRecord) -> AppResult<()> {
        let saved = self.decisions.record(&record);
        self.recent.push_front(record);
        self.recent.truncate(RECENT);
        saved
    }
}

/// The router's pick, or the fallback when it failed or picked badly.
fn on_router(pending: &Pending, reply: RouteReply) -> Routed {
    let cost = reply.cost_usd;
    let picked = reply
        .result
        .and_then(|answer| match find(&pending.candidates, &answer.session) {
            Some(session) => Ok((session.clone(), answer)),
            None => Err(format!("picked unknown session {:?}", answer.session)),
        });
    let mut routed = match picked {
        Ok((session, answer)) => Routed {
            ticket: pending.ticket,
            session,
            model_hint: answer.model,
            reason: answer.reason,
            source: Source::Haiku,
            confidence: None,
            cost_usd: 0.0,
        },
        Err(why) => fallback_route(
            pending,
            Source::HaikuError,
            format!("routing failed ({why}), kept the selected session"),
        ),
    };
    routed.cost_usd = cost;
    routed
}

fn fallback_route(pending: &Pending, source: Source, reason: String) -> Routed {
    Routed {
        ticket: pending.ticket,
        session: pending.fallback.clone(),
        model_hint: None,
        reason,
        source,
        confidence: None,
        cost_usd: 0.0,
    }
}

/// The candidate called `name` (exact, else ignoring case).
fn find<'a>(candidates: &'a [Session], name: &str) -> Option<&'a Session> {
    let name = name.trim();
    candidates.iter().find(|s| s.name == name).or_else(|| {
        candidates
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
    })
}

#[cfg(test)]
mod tests;
