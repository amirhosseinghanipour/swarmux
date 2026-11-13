//! The verification figures of the last hours, read from the ledger.
//!
//! The figures themselves are the domain's
//! ([`swarmux_domain::console::compute_metrics`]); this reads the ledger's
//! newest entries back to the start of the window and keeps the result for
//! a while, since the ledger grows with every agent event and the console
//! asks every frame.
//!
//! # Cost
//!
//! The window is read newest first, in pages, until an entry older than
//! the window turns up, the ledger starts, or [`MAX_ENTRIES`] have been
//! read. In the last case the figures say they describe only the newest
//! part of the window (`complete` is false) instead of passing for the
//! whole of it. The result is kept for [`REFRESH_MS`]: a busy ledger is
//! read a few times a minute, not per frame.

use std::rc::Rc;

use swarmux_domain::Millis;
use swarmux_domain::console::{Metrics, compute_metrics};
use swarmux_domain::ledger::LedgerEntry;

use crate::AppResult;
use crate::ledger::{Ledger, LedgerService};

/// Entries read per page.
const PAGE: usize = 500;
/// Most entries read for one set of figures.
pub const MAX_ENTRIES: usize = 20_000;
/// How long the figures are kept before the ledger is read again.
pub const REFRESH_MS: Millis = 20_000;
/// The window, in hours, when `[console] metrics_hours` says nothing.
pub const DEFAULT_HOURS: u32 = 24;
/// The longest window `[console] metrics_hours` may ask for: thirty days.
pub const MAX_HOURS: u32 = 720;

/// The newest entries of `ledger` back to `since`, oldest first, and
/// whether the read reached `since` (or the ledger's first entry) rather
/// than stopping at `max` entries.
pub fn read_window(
    ledger: &dyn Ledger,
    since: Millis,
    max: usize,
) -> AppResult<(Vec<LedgerEntry>, bool)> {
    let Some(head) = ledger.head()? else {
        return Ok((Vec::new(), true));
    };
    let mut newest_first: Vec<Vec<LedgerEntry>> = Vec::new();
    let mut read = 0usize;
    let mut to = head.seq;
    let reached = loop {
        let from = to.saturating_sub(PAGE as u64 - 1).max(1);
        let page = ledger.range(from, usize::try_from(to - from + 1).unwrap_or(PAGE))?;
        read += page.len();
        let before_window = page.first().is_some_and(|e| e.draft.ts < since);
        newest_first.push(page);
        if before_window || from == 1 {
            break true;
        }
        if read >= max {
            break false;
        }
        to = from - 1;
    };
    let entries = newest_first.into_iter().rev().flatten().collect();
    Ok((entries, reached))
}

/// Works out the figures for the window ending at `now`.
pub fn read_metrics(ledger: &dyn Ledger, now: Millis, window_ms: Millis) -> AppResult<Metrics> {
    let (entries, reached) = read_window(ledger, now.saturating_sub(window_ms), MAX_ENTRIES)?;
    Ok(compute_metrics(&entries, now, window_ms, reached))
}

/// Keeps the figures between reads.
pub struct MetricsService {
    ledger: Rc<LedgerService>,
    window_ms: Millis,
    last: Option<Metrics>,
    /// When the ledger was last read, successfully or not.
    read_at: Option<Millis>,
}

impl MetricsService {
    /// Figures over the last `hours` hours (`1..=`[`MAX_HOURS`]).
    pub fn new(ledger: Rc<LedgerService>, hours: u32) -> MetricsService {
        let hours = hours.clamp(1, MAX_HOURS);
        MetricsService {
            ledger,
            window_ms: Millis::from(hours) * 3_600_000,
            last: None,
            read_at: None,
        }
    }

    /// The figures as of `now`: the ones kept, until [`REFRESH_MS`] passed
    /// since they were worked out, then read again. A ledger that cannot
    /// be read leaves the old figures (or none) and is tried again at the
    /// next refresh.
    pub fn current(&mut self, now: Millis) -> Option<&Metrics> {
        let stale = self
            .read_at
            .is_none_or(|at| !(0..REFRESH_MS).contains(&(now - at)));
        if stale {
            self.read_at = Some(now);
            if let Ok(m) = read_metrics(self.ledger.ledger(), now, self.window_ms) {
                self.last = Some(m);
            }
        }
        self.last.as_ref()
    }

    /// The window the figures are over, in milliseconds.
    pub fn window_ms(&self) -> Millis {
        self.window_ms
    }
}

#[cfg(test)]
mod tests;
