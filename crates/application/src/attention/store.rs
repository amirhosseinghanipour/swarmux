//! What the inbox keeps across runs: the alarm log, shelved items and how
//! each kind of ask has been answered.

use swarmux_domain::alarm::Tally;
use swarmux_domain::{AttentionId, JobId, Millis, Priority};

use crate::AppResult;

/// What happened to an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlarmKind {
    /// It entered the inbox.
    Raised,
    /// The user first looked at it; the detail is the time that took.
    Attended,
    Shelved,
    /// A shelved item came back (time, condition, or by hand).
    Returned,
    /// Several asks were answered with one key.
    Batch,
    /// An irreversible ask was confirmed by naming its target.
    Confirmed,
    /// A rule was written from a run of identical approvals.
    Rule,
}

impl AlarmKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AlarmKind::Raised => "raised",
            AlarmKind::Attended => "attended",
            AlarmKind::Shelved => "shelved",
            AlarmKind::Returned => "returned",
            AlarmKind::Batch => "batch",
            AlarmKind::Confirmed => "confirmed",
            AlarmKind::Rule => "rule",
        }
    }

    pub fn parse(s: &str) -> Option<AlarmKind> {
        Some(match s {
            "raised" => AlarmKind::Raised,
            "attended" => AlarmKind::Attended,
            "shelved" => AlarmKind::Shelved,
            "returned" => AlarmKind::Returned,
            "batch" => AlarmKind::Batch,
            "confirmed" => AlarmKind::Confirmed,
            "rule" => AlarmKind::Rule,
            _ => return None,
        })
    }
}

/// One line of the alarm log.
#[derive(Clone, Debug, PartialEq)]
pub struct AlarmEvent {
    pub at: Millis,
    pub kind: AlarmKind,
    /// The item's key.
    pub key: String,
    pub priority: Option<Priority>,
    pub detail: String,
}

/// An item put aside for a while.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShelvedItem {
    pub id: AttentionId,
    pub job: Option<JobId>,
    pub shelved_at: Millis,
    /// When it comes back by itself.
    pub until: Option<Millis>,
    /// It also comes back once its job is no longer running.
    pub until_job_ends: bool,
}

/// How an item may be shelved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShelveFor {
    Minutes(u32),
    /// Until the item's job has ended.
    JobEnds,
}

/// A signature's tally in one repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TallyRow {
    pub scope: String,
    pub signature: String,
    pub tally: Tally,
}

/// Keeps the inbox's memory.
pub trait AlarmStore {
    fn log(&self, event: &AlarmEvent) -> AppResult<()>;
    /// Events at or after `since`, oldest first.
    fn events(&self, since: Millis) -> AppResult<Vec<AlarmEvent>>;

    fn save_shelved(&self, item: &ShelvedItem) -> AppResult<()>;
    fn remove_shelved(&self, id: &AttentionId) -> AppResult<()>;
    fn shelved(&self) -> AppResult<Vec<ShelvedItem>>;

    fn tallies(&self) -> AppResult<Vec<TallyRow>>;
    fn save_tally(&self, row: &TallyRow) -> AppResult<()>;
}

/// An in-memory [`AlarmStore`] (feature `test-support`).
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use std::cell::RefCell;

    use super::*;

    #[derive(Default)]
    pub struct InMemoryAlarms {
        log: RefCell<Vec<AlarmEvent>>,
        shelved: RefCell<Vec<ShelvedItem>>,
        tallies: RefCell<Vec<TallyRow>>,
    }

    impl InMemoryAlarms {
        pub fn log_of(&self, kind: AlarmKind) -> Vec<AlarmEvent> {
            self.log
                .borrow()
                .iter()
                .filter(|e| e.kind == kind)
                .cloned()
                .collect()
        }
    }

    impl AlarmStore for InMemoryAlarms {
        fn log(&self, event: &AlarmEvent) -> AppResult<()> {
            self.log.borrow_mut().push(event.clone());
            Ok(())
        }

        fn events(&self, since: Millis) -> AppResult<Vec<AlarmEvent>> {
            Ok(self
                .log
                .borrow()
                .iter()
                .filter(|e| e.at >= since)
                .cloned()
                .collect())
        }

        fn save_shelved(&self, item: &ShelvedItem) -> AppResult<()> {
            let mut rows = self.shelved.borrow_mut();
            rows.retain(|r| r.id != item.id);
            rows.push(item.clone());
            Ok(())
        }

        fn remove_shelved(&self, id: &AttentionId) -> AppResult<()> {
            self.shelved.borrow_mut().retain(|r| &r.id != id);
            Ok(())
        }

        fn shelved(&self) -> AppResult<Vec<ShelvedItem>> {
            Ok(self.shelved.borrow().clone())
        }

        fn tallies(&self) -> AppResult<Vec<TallyRow>> {
            Ok(self.tallies.borrow().clone())
        }

        fn save_tally(&self, row: &TallyRow) -> AppResult<()> {
            let mut rows = self.tallies.borrow_mut();
            rows.retain(|r| !(r.scope == row.scope && r.signature == row.signature));
            rows.push(row.clone());
            Ok(())
        }
    }
}
