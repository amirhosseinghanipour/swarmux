//! Follow-through: collecting what agents leave behind when they finish,
//! deciding it with the owner's standing orders, and bringing the owner
//! only what needs them, in batches.
//!
//! A model is used for one thing: reading a finished job's last message
//! and listing the follow-ups in it ([`Extractor`]). Everything after that
//! (dedupe, standing orders, urgency, the brief, the approval-rate check)
//! is deterministic code in `swarmux_domain::followup`.
//!
//! Approved follow-ups become [`Intent`]s: "approved, not yet a mission".
//! The backlog that turns intents into missions comes later; until then
//! the owner opens the mission composer with one key.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};

use swarmux_domain::board::{BoardEntry, BoardKind};
use swarmux_domain::followup::{
    BriefInput, BriefSlots, DecidedBy, FollowKind, FollowStatus, FollowUp, Matched, OwnerBrief,
    Source, StandingOrder, Urgency, compose, decide, due_slot, find_duplicate, pick,
    rubber_stamped,
};
use swarmux_domain::{AttentionItem, JobId, JobStatus, Millis, Priority};

use crate::gate::GateState;
use crate::jobs::JobFact;
use crate::ledger::LedgerService;
use crate::ports::Clock;
use crate::{AppError, AppResult};
use swarmux_domain::gate::GateOutcome;

const MINUTE: Millis = 60_000;
const HOUR: Millis = 60 * MINUTE;
const DAY: Millis = 24 * HOUR;
/// Work is looked for at most this often.
const THROTTLE_MS: Millis = 2_000;
/// Extraction calls in flight at once.
const MAX_EXTRACTING: usize = 2;
/// Follow-ups one job may add.
const PER_JOB_MAX: usize = 8;
/// Longest tail of a final message sent to the extractor, in characters.
pub const MESSAGE_MAX: usize = 6_000;

/// Where follow-ups are kept. Implemented by the database.
pub trait FollowStore {
    /// Stores a new follow-up; returns its id.
    fn insert(&self, f: &FollowUp) -> AppResult<i64>;
    fn update(&self, f: &FollowUp) -> AppResult<()>;
    fn get(&self, id: i64) -> AppResult<Option<FollowUp>>;
    /// The newest follow-ups, oldest first.
    fn all(&self) -> AppResult<Vec<FollowUp>>;
    /// A number kept between runs (the last brief, the jobs looked at).
    fn state(&self, key: &str) -> AppResult<Option<i64>>;
    fn set_state(&self, key: &str, value: i64) -> AppResult<()>;
}

/// An approved follow-up that is not yet a mission.
#[derive(Clone, Debug, PartialEq)]
pub struct Intent {
    /// Assigned by the store; `0` before.
    pub id: i64,
    pub followup: i64,
    pub text: String,
    pub repo: String,
    pub paths: Vec<String>,
    pub created: Millis,
    /// The mission it became, once it did.
    pub mission: Option<String>,
}

/// Where approved follow-ups wait. The Backlog of a later milestone reads
/// from here and turns them into missions.
pub trait IntentStore {
    fn add(&self, intent: &Intent) -> AppResult<i64>;
    /// Newest last.
    fn list(&self) -> AppResult<Vec<Intent>>;
    fn mark_mission(&self, id: i64, slug: &str) -> AppResult<()>;
}

/// What the extractor reads: only the job's goal, its final message and
/// the gate's verdict.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtractRequest {
    pub job: JobId,
    pub goal: String,
    pub final_message: String,
    pub gate: String,
}

/// One follow-up the extractor found.
#[derive(Clone, Debug, PartialEq)]
pub struct Extracted {
    pub kind: FollowKind,
    pub text: String,
    pub rationale: String,
    pub against: String,
    pub paths: Vec<String>,
    pub urgency: Urgency,
    pub est_cost_usd: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExtractReply {
    pub result: Result<Vec<Extracted>, String>,
    pub cost_usd: f64,
}

/// Receives the extraction, from any thread.
pub type ExtractSink = Box<dyn FnOnce(ExtractReply) + Send + 'static>;

/// Lists the follow-ups in a finished job's last message. Never blocks;
/// the answer arrives through the sink.
pub trait Extractor {
    fn extract(&self, request: ExtractRequest, sink: ExtractSink);
}

/// Carries a brief to the owner: a notification, a message. Adapters for
/// other places (a chat) implement this.
pub trait BriefChannel {
    fn deliver(&self, brief: &OwnerBrief) -> AppResult<()>;
}

/// What the owner can answer a card with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    Approve,
    Decline,
    Defer,
}

/// A reply that arrived from somewhere other than the TUI or the CLI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerReply {
    pub followup: i64,
    pub reply: Reply,
}

/// Where replies arrive from: a chat adapter polls its messages here.
pub trait ReplySource {
    fn poll(&self) -> Vec<OwnerReply>;
}

/// The `[followups]` section.
#[derive(Clone, Debug, PartialEq)]
pub struct FollowSettings {
    /// Off: nothing is collected from the Board or from finished jobs.
    pub enabled: bool,
    /// Ask a small model for the follow-ups in each finished job's message.
    pub extract: bool,
    /// The model that reads finished jobs.
    pub extract_model: String,
    pub slots: BriefSlots,
    pub orders: Vec<StandingOrder>,
    pub max_cards: usize,
    /// How long "later" is.
    pub defer_hours: u32,
    /// Days between spot-check prompts.
    pub spot_check_days: u32,
    /// Protected paths on top of the built-in ones.
    pub protected: Vec<String>,
}

impl Default for FollowSettings {
    fn default() -> FollowSettings {
        FollowSettings {
            enabled: true,
            extract: true,
            extract_model: "haiku".into(),
            slots: BriefSlots::default(),
            orders: Vec::new(),
            max_cards: 5,
            defer_hours: 24,
            spot_check_days: 7,
            protected: Vec::new(),
        }
    }
}

/// What one [`FollowUpService::pump`] did.
#[derive(Debug, Default)]
pub struct FollowPump {
    pub errors: Vec<AppError>,
    /// Briefs that came due and were delivered.
    pub briefs: Vec<OwnerBrief>,
    pub changed: bool,
}

/// Collects, decides and reports follow-ups.
pub struct FollowUpService {
    store: Rc<dyn FollowStore>,
    intents: Rc<dyn IntentStore>,
    clock: Rc<dyn Clock>,
    settings: FollowSettings,
    ledger: Option<Rc<LedgerService>>,
    extractor: Option<Rc<dyn Extractor>>,
    channels: Vec<Rc<dyn BriefChannel>>,
    replies: Vec<Rc<dyn ReplySource>>,
    repo_of: Option<Rc<dyn Fn(JobId) -> String>>,
    /// Minutes east of UTC now.
    utc_offset: fn() -> i32,
    tx: Sender<(JobId, ExtractReply)>,
    rx: Receiver<(JobId, ExtractReply)>,
    inflight: RefCell<HashSet<JobId>>,
    urgent: RefCell<Vec<AttentionItem>>,
    last_pump: Cell<Millis>,
}

impl FollowUpService {
    pub fn new(
        store: Rc<dyn FollowStore>,
        intents: Rc<dyn IntentStore>,
        clock: Rc<dyn Clock>,
        settings: FollowSettings,
    ) -> FollowUpService {
        let (tx, rx) = channel();
        FollowUpService {
            store,
            intents,
            clock,
            settings,
            ledger: None,
            extractor: None,
            channels: Vec::new(),
            replies: Vec::new(),
            repo_of: None,
            utc_offset: || 0,
            tx,
            rx,
            inflight: RefCell::new(HashSet::new()),
            urgent: RefCell::new(Vec::new()),
            last_pump: Cell::new(i64::MIN),
        }
    }

    /// Writes every discovery, decision and brief to the ledger.
    pub fn with_ledger(mut self, ledger: Rc<LedgerService>) -> FollowUpService {
        self.ledger = Some(ledger);
        self
    }

    pub fn with_extractor(mut self, extractor: Rc<dyn Extractor>) -> FollowUpService {
        self.extractor = Some(extractor);
        self
    }

    pub fn with_channel(mut self, channel: Rc<dyn BriefChannel>) -> FollowUpService {
        self.channels.push(channel);
        self
    }

    pub fn with_replies(mut self, source: Rc<dyn ReplySource>) -> FollowUpService {
        self.replies.push(source);
        self
    }

    /// Names the repository or workspace a job works in.
    pub fn with_repo_of(mut self, f: Rc<dyn Fn(JobId) -> String>) -> FollowUpService {
        self.repo_of = Some(f);
        self
    }

    pub fn with_utc_offset(mut self, f: fn() -> i32) -> FollowUpService {
        self.utc_offset = f;
        self
    }

    pub fn settings(&self) -> &FollowSettings {
        &self.settings
    }

    pub fn orders(&self) -> &[StandingOrder] {
        &self.settings.orders
    }

    fn now(&self) -> Millis {
        self.clock.now()
    }

    fn note(&self, event: &str, f: &FollowUp, by: &str, detail: &str, cost: f64) {
        if let Some(l) = &self.ledger {
            // The ledger is the record, but a full disk must not lose the
            // follow-up itself: it was stored first.
            let _ = l.record_followup(
                event,
                f.id,
                f.kind.as_str(),
                by,
                detail,
                f.first_job(),
                cost,
            );
        }
    }

    // --- collecting -------------------------------------------------

    /// Stores a follow-up, or merges it into the one it repeats, then lets
    /// the standing orders decide. `running` says the job it blocks still
    /// runs. Returns the id it is kept under.
    pub fn ingest(&self, f: FollowUp, running: bool) -> AppResult<i64> {
        self.ingest_with(f, running, true)
    }

    fn ingest_with(&self, mut f: FollowUp, running: bool, apply_orders: bool) -> AppResult<i64> {
        if f.repo.is_empty()
            && let (Some(job), Some(repo_of)) = (f.first_job(), &self.repo_of)
        {
            f.repo = repo_of(job);
        }
        let known = self.store.all()?;
        if let Some(i) = find_duplicate(&known, &f) {
            let mut kept = known[i].clone();
            let before = kept.sources.len();
            kept.merge(&f);
            if kept.sources.len() != before {
                self.store.update(&kept)?;
                self.note(
                    "merged",
                    &kept,
                    "swarmux",
                    &format!("{} sources", kept.sources.len()),
                    0.0,
                );
            }
            return Ok(kept.id);
        }
        if apply_orders {
            self.apply_orders(&mut f, running);
        }
        f.id = self.store.insert(&f)?;
        self.note("found", &f, "swarmux", f.status.as_str(), 0.0);
        if f.status.is_approved() {
            self.record_decision(&f);
            self.make_intent(&f)?;
        }
        Ok(f.id)
    }

    fn apply_orders(&self, f: &mut FollowUp, running: bool) {
        let m = decide(
            f,
            &self.settings.orders,
            f.urgent(running),
            &self.settings.protected,
        );
        let now = self.now();
        match m {
            Matched::Approve(id) => {
                f.status = FollowStatus::AutoApproved;
                f.decided_by = Some(DecidedBy::Order(id));
                f.decided_at = Some(now);
            }
            Matched::Park(id) => {
                f.status = FollowStatus::Deferred;
                f.due_at = None;
                f.decided_by = Some(DecidedBy::Order(id));
                f.decided_at = Some(now);
            }
            Matched::Ask { .. } | Matched::None => {}
        }
    }

    /// Ledgers an automatic decision.
    fn record_decision(&self, f: &FollowUp) {
        let Some(by) = &f.decided_by else { return };
        self.note(
            f.status.as_str(),
            f,
            &by.to_text(),
            "decided by standing order",
            0.0,
        );
    }

    fn make_intent(&self, f: &FollowUp) -> AppResult<Option<Intent>> {
        if f.kind == FollowKind::Question {
            return Ok(None);
        }
        let mut intent = Intent {
            id: 0,
            followup: f.id,
            text: f.text.clone(),
            repo: f.repo.clone(),
            paths: f.paths.clone(),
            created: self.now(),
            mission: None,
        };
        intent.id = self.intents.add(&intent)?;
        Ok(Some(intent))
    }

    /// Turns the Board's `schedule_followup` and `ask_human` entries into
    /// follow-ups, and closes the questions that were answered there.
    /// A follow-up with `after_minutes` is held until then.
    pub fn sync_board(
        &self,
        entries: &[BoardEntry],
        running: &dyn Fn(JobId) -> bool,
    ) -> AppResult<bool> {
        let mut known = self.store.all()?;
        let seen: HashSet<i64> = known
            .iter()
            .flat_map(|f| f.sources.iter().filter_map(|s| s.board_entry))
            .collect();
        let mut changed = false;
        for e in entries {
            let kind = match e.kind {
                BoardKind::Followup => FollowKind::SuggestedTask,
                BoardKind::Question => FollowKind::Question,
                _ => continue,
            };
            if seen.contains(&e.id) {
                if kind == FollowKind::Question && e.closed {
                    for f in known.iter_mut().filter(|f| {
                        f.kind == kind
                            && f.status != FollowStatus::Done
                            && f.sources.iter().any(|s| s.board_entry == Some(e.id))
                    }) {
                        f.status = FollowStatus::Done;
                        self.store.update(f)?;
                        changed = true;
                    }
                }
                continue;
            }
            if e.closed || e.text.is_empty() {
                continue;
            }
            let mut f = FollowUp::new(
                kind,
                &e.text,
                Source {
                    job: Some(e.job),
                    mission: Some(e.mission.clone()).filter(|m| !m.is_empty()),
                    board_entry: Some(e.id),
                },
                e.at,
            );
            f.rationale = e.detail.clone();
            f.paths = e.areas.clone();
            let delayed = e.kind == BoardKind::Followup && e.after_minutes.is_some_and(|m| m > 0);
            if delayed {
                f.status = FollowStatus::Deferred;
                f.due_at = Some(e.at + Millis::from(e.after_minutes.unwrap_or(0)) * MINUTE);
            }
            self.ingest_with(f, running(e.job), !delayed)?;
            changed = true;
        }
        Ok(changed)
    }

    // --- deciding ---------------------------------------------------

    /// Everything waiting for the owner, oldest first.
    pub fn waiting(&self) -> AppResult<Vec<FollowUp>> {
        let now = self.now();
        Ok(self
            .store
            .all()?
            .into_iter()
            .filter(|f| f.waiting(now))
            .collect())
    }

    pub fn all(&self) -> AppResult<Vec<FollowUp>> {
        self.store.all()
    }

    pub fn get(&self, id: i64) -> AppResult<Option<FollowUp>> {
        self.store.get(id)
    }

    pub fn intents(&self) -> AppResult<Vec<Intent>> {
        self.intents.list()
    }

    /// The owner's answer to a card. An approval of anything but a
    /// question leaves an [`Intent`] behind, returned.
    pub fn reply(&self, id: i64, reply: Reply) -> AppResult<Option<Intent>> {
        let mut f = self
            .store
            .get(id)?
            .ok_or_else(|| AppError::Mission(format!("no follow-up #{id}")))?;
        if matches!(f.status, FollowStatus::Done) {
            return Err(AppError::Mission(format!(
                "follow-up #{id} is already done"
            )));
        }
        if reply == Reply::Approve && f.kind == FollowKind::Question {
            return Err(AppError::Mission(format!(
                "#{id} is a question: answer it on the Board (key 7), not approve it"
            )));
        }
        let now = self.now();
        f.decided_by = Some(DecidedBy::Owner);
        f.decided_at = Some(now);
        f.due_at = None;
        f.status = match reply {
            Reply::Approve => FollowStatus::Approved,
            Reply::Decline => FollowStatus::Declined,
            Reply::Defer => {
                f.due_at = Some(now + Millis::from(self.settings.defer_hours) * HOUR);
                FollowStatus::Deferred
            }
        };
        self.store.update(&f)?;
        self.note(f.status.as_str(), &f, "owner", "", 0.0);
        let intent = if reply == Reply::Approve {
            self.make_intent(&f)?
        } else {
            None
        };
        self.refresh_urgent(&|_| false);
        Ok(intent)
    }

    /// Records the mission an intent became.
    pub fn intent_became(&self, intent: i64, slug: &str) -> AppResult<()> {
        self.intents.mark_mission(intent, slug)
    }

    // --- the inbox --------------------------------------------------

    /// Urgent follow-ups, for Mission control: only these interrupt.
    pub fn urgent_items(&self) -> Vec<AttentionItem> {
        self.urgent.borrow().clone()
    }

    fn refresh_urgent(&self, running: &dyn Fn(JobId) -> bool) {
        let Ok(all) = self.store.all() else { return };
        let now = self.now();
        let items = all
            .iter()
            .filter(|f| f.waiting(now))
            .filter(|f| f.urgent(f.jobs().into_iter().any(running)))
            .map(|f| {
                let p1 = f.kind == FollowKind::Risk || f.urgency == Urgency::High;
                AttentionItem::follow_up(
                    f.id,
                    f.first_job(),
                    if p1 { Priority::P1 } else { Priority::P2 },
                    &f.text,
                    &format!(
                        "{} · {}",
                        f.kind.as_str(),
                        f.sources.first().map_or("—".into(), Source::line)
                    ),
                    f.created,
                )
            })
            .collect();
        *self.urgent.borrow_mut() = items;
    }

    // --- the pump ---------------------------------------------------

    /// Looks for new follow-ups (Board entries, finished jobs), wakes the
    /// deferred ones that are due, takes outside replies and sends the
    /// brief when its time has come.
    pub fn pump(
        &self,
        facts: &[JobFact],
        gates: &HashMap<JobId, GateState>,
        board: &[BoardEntry],
    ) -> FollowPump {
        let mut out = FollowPump::default();
        if !self.settings.enabled {
            return out;
        }
        let now = self.now();
        if now.saturating_sub(self.last_pump.get()) < THROTTLE_MS {
            return out;
        }
        self.last_pump.set(now);
        let running = |job: JobId| {
            facts
                .iter()
                .any(|f| f.job_id == job && f.status == JobStatus::Running)
        };
        let mut step = |r: AppResult<bool>| match r {
            Ok(c) => out.changed |= c,
            Err(e) => out.errors.push(e),
        };
        step(self.sync_board(board, &running));
        step(self.wake_due());
        step(self.extract_finished(facts, gates));
        step(self.take_extractions(&running));
        step(self.take_replies());
        match self.brief_if_due(facts, gates) {
            Ok(Some(b)) => {
                out.changed = true;
                out.briefs.push(b);
            }
            Ok(None) => {}
            Err(e) => out.errors.push(e),
        }
        self.refresh_urgent(&running);
        out
    }

    /// Deferred follow-ups whose time came open again; the standing orders
    /// see them then.
    fn wake_due(&self) -> AppResult<bool> {
        let now = self.now();
        let mut changed = false;
        for mut f in self.store.all()? {
            if f.status == FollowStatus::Deferred && f.due_at.is_some_and(|d| d <= now) {
                f.status = FollowStatus::Open;
                f.due_at = None;
                // A follow-up the owner put off goes back to the owner.
                if f.decided_by != Some(DecidedBy::Owner) {
                    self.apply_orders(&mut f, false);
                }
                self.store.update(&f)?;
                self.note("due", &f, "swarmux", f.status.as_str(), 0.0);
                if f.status.is_approved() {
                    self.record_decision(&f);
                    self.make_intent(&f)?;
                }
                changed = true;
            }
        }
        Ok(changed)
    }

    fn take_replies(&self) -> AppResult<bool> {
        let mut changed = false;
        for source in &self.replies {
            for r in source.poll() {
                self.reply(r.followup, r.reply)?;
                changed = true;
            }
        }
        Ok(changed)
    }

    // --- extraction -------------------------------------------------

    fn extract_finished(
        &self,
        facts: &[JobFact],
        gates: &HashMap<JobId, GateState>,
    ) -> AppResult<bool> {
        let Some(extractor) = self.extractor.as_ref().filter(|_| self.settings.extract) else {
            return Ok(false);
        };
        let since = match self.store.state("since")? {
            Some(s) => s,
            None => {
                self.store.set_state("since", self.now())?;
                self.now()
            }
        };
        for fact in facts {
            if self.inflight.borrow().len() >= MAX_EXTRACTING {
                break;
            }
            let finished = fact.status == JobStatus::Done
                && fact.review_of.is_none()
                && fact.finished_at.is_some_and(|t| t >= since);
            let Some(message) = fact
                .result
                .as_deref()
                .filter(|m| finished && !m.trim().is_empty())
            else {
                continue;
            };
            let key = format!("job:{}", fact.job_id);
            if self.store.state(&key)?.is_some() || self.inflight.borrow().contains(&fact.job_id) {
                continue;
            }
            self.store.set_state(&key, 1)?;
            let request = ExtractRequest {
                job: fact.job_id,
                goal: fact.prompt.chars().take(600).collect(),
                final_message: tail(message, MESSAGE_MAX),
                gate: gate_line(gates.get(&fact.job_id)),
            };
            self.inflight.borrow_mut().insert(fact.job_id);
            let tx = self.tx.clone();
            let job = fact.job_id;
            extractor.extract(
                request,
                Box::new(move |reply| {
                    let _ = tx.send((job, reply));
                }),
            );
        }
        Ok(false)
    }

    fn take_extractions(&self, running: &dyn Fn(JobId) -> bool) -> AppResult<bool> {
        let mut changed = false;
        while let Ok((job, reply)) = self.rx.try_recv() {
            self.inflight.borrow_mut().remove(&job);
            let now = self.now();
            let mut found = 0;
            let detail = match reply.result {
                Ok(list) => {
                    for x in list.into_iter().take(PER_JOB_MAX) {
                        let mut f = FollowUp::new(
                            x.kind,
                            &x.text,
                            Source {
                                job: Some(job),
                                ..Source::default()
                            },
                            now,
                        );
                        if f.text.is_empty() {
                            continue;
                        }
                        f.rationale = swarmux_domain::board::clean(
                            &x.rationale,
                            swarmux_domain::followup::NOTE_MAX,
                        );
                        f.against = swarmux_domain::board::clean(
                            &x.against,
                            swarmux_domain::followup::NOTE_MAX,
                        );
                        f.paths = x.paths;
                        f.urgency = x.urgency;
                        f.est_cost_usd = x.est_cost_usd.filter(|c| c.is_finite() && *c >= 0.0);
                        self.ingest(f, running(job))?;
                        found += 1;
                    }
                    format!("{found} found")
                }
                Err(why) => format!("failed: {why}"),
            };
            if let Some(l) = &self.ledger {
                let _ = l.record_followup_extraction(job, &detail, reply.cost_usd);
            }
            changed = true;
        }
        Ok(changed)
    }

    // --- the brief --------------------------------------------------

    fn owner_decisions(&self, all: &[FollowUp]) -> Vec<(FollowKind, bool)> {
        let mut d: Vec<(&FollowUp, bool)> = all
            .iter()
            .filter(|f| f.decided_by == Some(DecidedBy::Owner))
            .filter_map(|f| match f.status {
                FollowStatus::Approved => Some((f, true)),
                FollowStatus::Declined => Some((f, false)),
                _ => None,
            })
            .collect();
        d.sort_by_key(|(f, _)| std::cmp::Reverse(f.decided_at));
        d.into_iter().map(|(f, a)| (f.kind, a)).collect()
    }

    /// Builds the brief for now. `finished`/`met` count the jobs that ended
    /// since `since`.
    pub fn compose_brief(
        &self,
        facts: &[JobFact],
        gates: &HashMap<JobId, GateState>,
        since: Millis,
        mark_spot_check: bool,
    ) -> AppResult<OwnerBrief> {
        let now = self.now();
        let all = self.store.all()?;
        let running = |job: JobId| {
            facts
                .iter()
                .any(|f| f.job_id == job && f.status == JobStatus::Running)
        };
        let waiting: Vec<(FollowUp, bool)> = all
            .iter()
            .filter(|f| f.waiting(now))
            .map(|f| (f.clone(), f.urgent(f.jobs().into_iter().any(running))))
            .collect();
        let ended: Vec<&JobFact> = facts
            .iter()
            .filter(|f| {
                f.review_of.is_none()
                    && f.status == JobStatus::Done
                    && f.finished_at.is_some_and(|t| t > since && t <= now)
            })
            .collect();
        let met = ended
            .iter()
            .filter(|f| {
                gates
                    .get(&f.job_id)
                    .is_some_and(|g| g.outcome == GateOutcome::Pass)
            })
            .count();
        let stamped = rubber_stamped(&self.owner_decisions(&all));
        let mut spot = None;
        let last_spot = self.store.state("spot")?.unwrap_or(0);
        if !stamped.is_empty()
            && now - last_spot >= Millis::from(self.settings.spot_check_days) * DAY
        {
            let kinds: Vec<FollowKind> = stamped.iter().map(|s| s.kind).collect();
            let candidates: Vec<&FollowUp> = all
                .iter()
                .filter(|f| {
                    f.status == FollowStatus::Approved
                        && f.decided_by == Some(DecidedBy::Owner)
                        && kinds.contains(&f.kind)
                })
                .collect();
            let ids: Vec<i64> = candidates.iter().map(|f| f.id).collect();
            if let Some(id) = pick(&ids, now)
                && let Some(f) = candidates.iter().find(|f| f.id == id)
            {
                spot = Some((id, f.text.clone()));
                if mark_spot_check {
                    self.store.set_state("spot", now)?;
                    self.note(
                        "spot_check",
                        f,
                        "swarmux",
                        "asked the owner to open the evidence",
                        0.0,
                    );
                }
            }
        }
        Ok(compose(&BriefInput {
            now,
            finished: ended.len(),
            met_checks: met,
            waiting: &waiting,
            max_cards: self.settings.max_cards,
            stamped: &stamped,
            spot_check: spot,
        }))
    }

    /// The brief on demand: since the last one, or the last day.
    pub fn brief_now(
        &self,
        facts: &[JobFact],
        gates: &HashMap<JobId, GateState>,
    ) -> AppResult<OwnerBrief> {
        let since = self
            .store
            .state("brief")?
            .unwrap_or_else(|| self.now() - DAY);
        self.compose_brief(facts, gates, since, true)
    }

    fn brief_if_due(
        &self,
        facts: &[JobFact],
        gates: &HashMap<JobId, GateState>,
    ) -> AppResult<Option<OwnerBrief>> {
        if self.settings.slots.is_empty() {
            return Ok(None);
        }
        let now = self.now();
        let Some(last) = self.store.state("brief")? else {
            // The first run starts the clock: no brief for the past.
            self.store.set_state("brief", now)?;
            return Ok(None);
        };
        let Some(slot) = due_slot(&self.settings.slots, now, (self.utc_offset)(), last) else {
            return Ok(None);
        };
        let brief = self.compose_brief(facts, gates, last, true)?;
        self.store.set_state("brief", slot)?;
        if brief.is_empty() && brief.finished == 0 {
            return Ok(None);
        }
        for c in &self.channels {
            // One failing channel must not hide the brief from the others.
            let _ = c.deliver(&brief);
        }
        if let Some(l) = &self.ledger {
            let _ = l.record_brief(
                brief.cards.len() + brief.more,
                brief.finished,
                brief.met_checks,
            );
        }
        Ok(Some(brief))
    }
}

fn tail(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    s.chars().skip(n - max).collect()
}

fn gate_line(g: Option<&GateState>) -> String {
    match g {
        None => "no gate verdict".into(),
        Some(g) => {
            let passed = g.checks.iter().filter(|c| c.1).count();
            format!(
                "gate {}: {passed} of {} checks passed",
                g.outcome.as_str(),
                g.checks.len()
            )
        }
    }
}

#[cfg(test)]
mod tests;
