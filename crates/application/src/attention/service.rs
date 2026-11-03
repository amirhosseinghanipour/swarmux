//! The inbox's alarm discipline: what is put aside, held back, answered
//! together or counted.

use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;

use swarmux_domain::alarm::{
    Breakpoint, Flood, Kpis, Raised, RiskTier, Signature, compute_kpis, deliver_by, releases,
};
use swarmux_domain::{AttentionId, AttentionItem, AttentionKind, Millis, Priority};

use super::store::{AlarmEvent, AlarmKind, AlarmStore, ShelveFor, ShelvedItem, TallyRow};
use super::{AttentionService, JobFact};
use crate::AppResult;
use swarmux_domain::JobStatus;

/// How far back the alarm log is read when it is opened.
const LOG_KEPT_MS: Millis = 24 * 60 * 60_000;

/// Items focus mode is holding back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeldBack {
    pub count: usize,
    /// The soonest an item must be shown by its bound, from now.
    pub next_in: Option<Millis>,
}

/// An offer to turn a run of identical approvals into a rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    /// The allow rule, in the Guard's syntax.
    pub rule: String,
    pub approved: u32,
}

impl AttentionService {
    /// Keeps the inbox's memory in `store` and loads what a previous run
    /// left: shelved items, how asks were answered and the last day's log.
    pub fn with_store(mut self, store: Rc<dyn AlarmStore>, now: Millis) -> AttentionService {
        let loaded = (|| -> AppResult<()> {
            self.shelved = store.shelved()?;
            for row in store.tallies()? {
                self.tallies.insert((row.scope, row.signature), row.tally);
            }
            for e in store.events(now - LOG_KEPT_MS)? {
                match e.kind {
                    AlarmKind::Raised => {
                        if let Some(priority) = e.priority {
                            self.raised.push(Raised { at: e.at, priority });
                        }
                        // Approval ids start over every run; job items keep
                        // theirs and must not be counted twice.
                        if !e.key.starts_with("approval:") {
                            self.raised_seen.insert(AttentionId::new(e.key));
                        }
                    }
                    AlarmKind::Attended => {
                        if let Ok(ms) = e.detail.parse::<Millis>() {
                            self.ttds.push(ms);
                        }
                        if !e.key.starts_with("approval:") {
                            self.attended.insert(AttentionId::new(e.key));
                        }
                    }
                    _ => {}
                }
            }
            Ok(())
        })();
        if let Err(e) = loaded {
            self.error = Some(e.to_string());
        }
        self.store = Some(store);
        self
    }

    /// The last storage problem, once.
    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    fn log(&mut self, event: AlarmEvent) {
        if let Some(store) = &self.store
            && let Err(e) = store.log(&event)
        {
            self.error = Some(e.to_string());
        }
    }

    fn event(
        &mut self,
        at: Millis,
        kind: AlarmKind,
        key: &AttentionId,
        p: Option<Priority>,
        detail: &str,
    ) {
        self.log(AlarmEvent {
            at,
            kind,
            key: key.to_string(),
            priority: p,
            detail: detail.to_string(),
        });
    }

    /// Counts the items that entered the inbox since the last look.
    pub(super) fn note_raised(&mut self, items: &[AttentionItem], now: Millis) {
        for i in items {
            if self.raised_seen.insert(i.id.clone()) {
                self.raised.push(Raised {
                    at: now,
                    priority: i.priority,
                });
                let detail = format!("{} {}", i.kind.as_str(), i.title);
                self.event(now, AlarmKind::Raised, &i.id, Some(i.priority), &detail);
            }
        }
        self.raised_seen
            .retain(|id| items.iter().any(|i| &i.id == id));
        self.raised.retain(|r| now - r.at < LOG_KEPT_MS);
    }

    // --- shelving ----------------------------------------------------------

    pub fn is_shelved(&self, id: &AttentionId) -> bool {
        self.shelved.iter().any(|s| &s.id == id)
    }

    /// What is put aside now.
    pub fn shelved_items(&self) -> &[ShelvedItem] {
        &self.shelved
    }

    /// Puts an item aside until its time is up or its job has ended; it
    /// comes back by itself. P1 can't be shelved. Returns whether it was.
    pub fn shelve(&mut self, item: &AttentionItem, how: ShelveFor, now: Millis) -> AppResult<bool> {
        if item.priority == Priority::P1 || self.is_shelved(&item.id) {
            return Ok(false);
        }
        let (until, until_job_ends) = match how {
            ShelveFor::Minutes(m) => (Some(now + Millis::from(m) * 60_000), false),
            ShelveFor::JobEnds => (None, true),
        };
        if until_job_ends && item.job.is_none() {
            return Ok(false);
        }
        let shelved = ShelvedItem {
            id: item.id.clone(),
            job: item.job,
            shelved_at: now,
            until,
            until_job_ends,
        };
        if let Some(store) = &self.store {
            store.save_shelved(&shelved)?;
        }
        let detail = match how {
            ShelveFor::Minutes(m) => format!("{m} min"),
            ShelveFor::JobEnds => "until its job ends".to_string(),
        };
        self.event(
            now,
            AlarmKind::Shelved,
            &item.id,
            Some(item.priority),
            &detail,
        );
        self.shelved.push(shelved);
        Ok(true)
    }

    /// Brings every shelved item back now. Returns how many.
    pub fn unshelve_all(&mut self, now: Millis) -> usize {
        let all = std::mem::take(&mut self.shelved);
        for s in &all {
            self.returned(s, now, "by hand");
        }
        all.len()
    }

    fn returned(&mut self, s: &ShelvedItem, now: Millis, why: &str) {
        if let Some(store) = &self.store
            && let Err(e) = store.remove_shelved(&s.id)
        {
            self.error = Some(e.to_string());
        }
        self.event(now, AlarmKind::Returned, &s.id, None, why);
    }

    /// Brings back what is due, and forgets what resolved on its own.
    pub(super) fn expire_shelf(&mut self, items: &[AttentionItem], facts: &[JobFact], now: Millis) {
        for s in std::mem::take(&mut self.shelved) {
            let present = items.iter().any(|i| i.id == s.id);
            let running = s.job.is_some_and(|j| {
                facts
                    .iter()
                    .any(|f| f.job_id == j && f.status == JobStatus::Running)
            });
            let why = if !present {
                Some("resolved")
            } else if s.until.is_some_and(|t| now >= t) {
                Some("time is up")
            } else if s.until_job_ends && !running {
                Some("its job ended")
            } else {
                None
            };
            match why {
                Some(why) => self.returned(&s, now, why),
                None => self.shelved.push(s),
            }
        }
    }

    // --- focus mode and deferral ---------------------------------------------

    pub fn focus(&self) -> bool {
        self.focus.is_some()
    }

    /// Turns focus mode on or off. Ending it lets everything through.
    pub fn set_focus(&mut self, on: bool, now: Millis) {
        if on {
            self.focus.get_or_insert(now);
        } else {
            self.focus = None;
            self.released.clear();
            self.held.clear();
            self.held_back = HeldBack::default();
        }
    }

    /// What focus mode holds back at the last collection.
    pub fn held_back(&self) -> HeldBack {
        self.held_back
    }

    /// A moment where interrupting costs little: in focus mode the held-back
    /// items that may come through do, at the next collection.
    pub fn breakpoint(&mut self, breakpoint: Breakpoint) {
        if self.focus.is_none() {
            return;
        }
        for (id, priority) in &self.held {
            if releases(*priority, breakpoint, true) {
                self.released.insert(id.clone());
            }
        }
    }

    /// P1 is always shown. In focus mode the rest waits for a breakpoint (P2)
    /// or for focus to end (P3), and never longer than its bound.
    pub(super) fn apply_focus(
        &mut self,
        items: Vec<AttentionItem>,
        now: Millis,
    ) -> Vec<AttentionItem> {
        self.held.clear();
        self.held_back = HeldBack::default();
        if self.focus.is_none() {
            self.released.clear();
            return items;
        }
        let mut shown = Vec::new();
        let mut soonest: Option<Millis> = None;
        for i in items {
            let due = deliver_by(i.since, i.priority, true);
            if i.priority == Priority::P1 || self.released.contains(&i.id) || now >= due {
                shown.push(i);
                continue;
            }
            soonest = Some(soonest.map_or(due - now, |s| s.min(due - now)));
            self.held.push((i.id.clone(), i.priority));
            self.held_back.count += 1;
        }
        self.held_back.next_in = soonest;
        let open: HashSet<&AttentionId> = shown.iter().map(|i| &i.id).collect();
        self.released.retain(|id| open.contains(id));
        shown
    }

    // --- batching ---------------------------------------------------------------

    /// Finds the asks that can be answered together: one signature, so one
    /// tool, one pattern and one tier. Destructive and P1 asks never are.
    pub(super) fn group_batches(&mut self, items: &mut [AttentionItem]) {
        self.members.clear();
        let mut groups: BTreeMap<(String, String, RiskTier), Vec<u64>> = BTreeMap::new();
        for i in items.iter() {
            let (Some(sig), Some(request)) = (&i.signature, i.request) else {
                continue;
            };
            if i.kind != AttentionKind::Approval
                || i.priority == Priority::P1
                || sig.tier == RiskTier::Destructive
            {
                continue;
            }
            groups
                .entry((sig.tool.clone(), sig.pattern.clone(), sig.tier))
                .or_default()
                .push(request);
        }
        for ids in groups.values_mut() {
            ids.sort_unstable();
        }
        for i in items.iter_mut() {
            let Some(sig) = &i.signature else { continue };
            let Some(request) = i.request else { continue };
            let key = (sig.tool.clone(), sig.pattern.clone(), sig.tier);
            if let Some(ids) = groups.get(&key)
                && ids.contains(&request)
            {
                i.batch = ids.len();
                self.members.insert(i.id.clone(), ids.clone());
            }
        }
    }

    /// The held calls that `A` answers along with `item`: its signature
    /// group, when that has more than one. Empty for P1 and destructive asks.
    pub fn batch_requests(&self, item: &AttentionItem) -> Vec<u64> {
        if item.priority == Priority::P1 || item.is_destructive_ask() {
            return Vec::new();
        }
        self.members
            .get(&item.id)
            .filter(|ids| ids.len() > 1)
            .cloned()
            .unwrap_or_default()
    }

    /// Logs that `count` asks were answered with one key.
    pub fn record_batch(
        &mut self,
        item: &AttentionItem,
        count: usize,
        approved: bool,
        now: Millis,
    ) {
        let label = item
            .signature
            .as_ref()
            .map(Signature::label)
            .unwrap_or_default();
        let verdict = if approved { "approved" } else { "denied" };
        self.event(
            now,
            AlarmKind::Batch,
            &item.id,
            Some(item.priority),
            &format!("{verdict} {count} × {label}"),
        );
    }

    /// Logs that an irreversible ask was confirmed by naming its target.
    pub fn record_confirmed(&mut self, item: &AttentionItem, target: &str, now: Millis) {
        self.event(
            now,
            AlarmKind::Confirmed,
            &item.id,
            Some(item.priority),
            target,
        );
    }

    // --- looking, answering, learning -------------------------------------------

    /// Notes that the user has now looked at `item`: the first time counts,
    /// and the time since it entered the inbox is logged.
    pub fn attend(&mut self, item: &AttentionItem, now: Millis) {
        if !self.attended.insert(item.id.clone()) {
            return;
        }
        let ttd = (now - item.since).max(0);
        self.ttds.push(ttd);
        self.event(
            now,
            AlarmKind::Attended,
            &item.id,
            Some(item.priority),
            &ttd.to_string(),
        );
    }

    /// Counts an answer to a held call, for the offer of a rule.
    pub fn answered(&mut self, item: &AttentionItem, approved: bool) {
        let (Some(sig), Some(scope)) = (&item.signature, self.scopes.get(&item.id)) else {
            return;
        };
        let key = (scope.clone(), sig.key());
        let tally = self.tallies.entry(key.clone()).or_default();
        if approved {
            tally.approved += 1;
        } else {
            tally.rejected += 1;
        }
        let row = TallyRow {
            scope: key.0,
            signature: key.1,
            tally: *tally,
        };
        self.save_tally(&row);
    }

    /// The open ask with this request id, shown or not.
    pub fn find_request(&self, request: u64) -> Option<&AttentionItem> {
        self.open.iter().find(|i| i.request == Some(request))
    }

    /// [`answered`](Self::answered) for the open ask with this request id.
    pub fn answered_request(&mut self, request: u64, approved: bool) {
        let item = self.open.iter().find(|i| i.request == Some(request));
        if let Some(item) = item.cloned() {
            self.answered(&item, approved);
        }
    }

    fn save_tally(&mut self, row: &TallyRow) {
        if let Some(store) = &self.store
            && let Err(e) = store.save_tally(row)
        {
            self.error = Some(e.to_string());
        }
    }

    /// The offer to make a rule of `item`'s signature, when it has earned one.
    pub fn suggestion(&self, item: &AttentionItem) -> Option<Suggestion> {
        let approved = item.suggest?;
        Some(Suggestion {
            rule: item.signature.as_ref()?.rule(),
            approved,
        })
    }

    /// The offer is taken (the caller writes the rule) or turned down: it is
    /// not made again for this signature here.
    pub fn settle_suggestion(&mut self, item: &AttentionItem, taken: bool, now: Millis) {
        let (Some(sig), Some(scope)) = (&item.signature, self.scopes.get(&item.id)) else {
            return;
        };
        let key = (scope.clone(), sig.key());
        let tally = self.tallies.entry(key.clone()).or_default();
        tally.suggested = true;
        let row = TallyRow {
            scope: key.0,
            signature: key.1,
            tally: *tally,
        };
        self.save_tally(&row);
        if taken {
            let rule = sig.rule();
            self.event(now, AlarmKind::Rule, &item.id, Some(item.priority), &rule);
        }
    }

    // --- figures ------------------------------------------------------------------

    /// The alarm key figures now.
    pub fn kpis(&self, now: Millis) -> Kpis {
        compute_kpis(&self.raised, &self.ttds, &self.open, now)
    }

    /// The flood among the items shown at the last collection, if any.
    pub fn flood(&self) -> Option<&Flood> {
        self.flood.as_ref()
    }
}

/// The alarm key figures from the log in `store`, as `swarmux stats alarms`
/// prints them. Nothing is open outside a run, so stale and standing are 0.
pub fn alarm_report(store: &dyn AlarmStore, now: Millis, days: i64) -> AppResult<Kpis> {
    let events = store.events(now - days * 24 * 60 * 60_000)?;
    let raised: Vec<Raised> = events
        .iter()
        .filter(|e| e.kind == AlarmKind::Raised)
        .filter_map(|e| {
            Some(Raised {
                at: e.at,
                priority: e.priority?,
            })
        })
        .collect();
    let ttds: Vec<Millis> = events
        .iter()
        .filter(|e| e.kind == AlarmKind::Attended)
        .filter_map(|e| e.detail.parse().ok())
        .collect();
    Ok(compute_kpis(&raised, &ttds, &[], now))
}
