//! State of the Ledger screen: the run cards, the search and filter, the
//! numbers over all runs, the digest of what happened while the user was
//! away, and the selected run's detail and bundle.

use swarmux_application::{JobDetail, JobSummary, Stored};
use swarmux_domain::JobId;
use swarmux_domain::evidence::{Brief, LabelKind, LedgerStats, RunCard, RunOutcome};

/// Days the heat strip covers.
pub const HEAT_DAYS: usize = 28;

/// Which runs the list shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    /// Missions with a bundle.
    Delivered,
    /// Runs that wait for the user.
    NeedsYou,
    /// Runs that earned an outcome label (merged, reverted, …).
    Labelled,
}

impl Filter {
    pub fn next(self) -> Filter {
        match self {
            Filter::All => Filter::Delivered,
            Filter::Delivered => Filter::NeedsYou,
            Filter::NeedsYou => Filter::Labelled,
            Filter::Labelled => Filter::All,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Delivered => "delivered",
            Filter::NeedsYou => "needs you",
            Filter::Labelled => "labelled",
        }
    }

    pub fn admits(self, card: &RunCard) -> bool {
        match self {
            Filter::All => true,
            Filter::Delivered => card.outcome == RunOutcome::Delivered,
            Filter::NeedsYou => card.outcome.needs_you(),
            Filter::Labelled => card
                .labels
                .last()
                .is_some_and(|l| l.kind != LabelKind::PrOpen),
        }
    }
}

/// What the user asks of the Ledger screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Search,
    /// The next filter.
    Filter,
    Labels,
    Prev,
    Next,
    Rerun,
    Fork,
    View,
    Verify,
}

/// What a command on the Ledger screen comes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LedgerAction {
    None,
    /// The selection moved; load its detail.
    Moved,
    Search,
    /// The filter changed; rebuild the list.
    Filtered,
    Rerun(JobId),
    Fork(JobId),
    View(JobId),
    /// Verify the bundle of this mission.
    Verify(String),
    /// Ask GitHub what became of the delivered work.
    Labels,
}

#[derive(Default)]
pub struct LedgerScreen {
    /// The jobs shown, newest first; `cards` has one card per row.
    pub rows: Vec<JobSummary>,
    pub cards: Vec<RunCard>,
    pub selected: usize,
    pub query: String,
    pub filter: Filter,
    pub detail: Option<JobDetail>,
    /// The bundle of the selected run, when it has one.
    pub bundle: Option<Stored>,
    /// Numbers over every run, whatever the search and filter.
    pub stats: LedgerStats,
    /// What happened in the digest's window.
    pub brief: Brief,
    /// The digest covers the time since the user was last here.
    pub away: bool,
    /// Runs per day, oldest day first.
    pub heat: Vec<u32>,
}

impl LedgerScreen {
    pub fn selected_row(&self) -> Option<&JobSummary> {
        self.rows.get(self.selected)
    }

    pub fn selected_card(&self) -> Option<&RunCard> {
        self.cards.get(self.selected)
    }

    /// Replaces the rows and their cards, keeping the selected job
    /// selected if it is still there.
    pub fn set_rows(&mut self, rows: Vec<JobSummary>, cards: Vec<RunCard>) {
        debug_assert_eq!(rows.len(), cards.len());
        let keep = self.selected_row().map(|r| r.job_id);
        self.rows = rows;
        self.cards = cards;
        if let Some(i) = keep.and_then(|id| self.rows.iter().position(|r| r.job_id == id)) {
            self.selected = i;
        }
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    pub fn handle(&mut self, command: Command) -> LedgerAction {
        match command {
            Command::Search => return LedgerAction::Search,
            Command::Filter => {
                self.filter = self.filter.next();
                self.selected = 0;
                return LedgerAction::Filtered;
            }
            Command::Labels => return LedgerAction::Labels,
            _ => {}
        }
        let Some(id) = self.selected_row().map(|r| r.job_id) else {
            return LedgerAction::None;
        };
        match command {
            Command::Prev if self.selected > 0 => {
                self.selected -= 1;
                LedgerAction::Moved
            }
            Command::Next if self.selected + 1 < self.rows.len() => {
                self.selected += 1;
                LedgerAction::Moved
            }
            Command::Rerun => LedgerAction::Rerun(id),
            Command::Fork => LedgerAction::Fork(id),
            Command::View => LedgerAction::View(id),
            Command::Verify => match self.selected_card().and_then(|c| c.mission.clone()) {
                Some(slug) if self.bundle.is_some() => LedgerAction::Verify(slug),
                _ => LedgerAction::None,
            },
            _ => LedgerAction::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_domain::{JobStatus, SessionId};

    fn row(id: i64) -> JobSummary {
        JobSummary {
            job_id: JobId(id),
            session_id: SessionId::random(),
            session_name: "s".into(),
            prompt: "p".into(),
            model: "m".into(),
            status: JobStatus::Done,
            cost_usd: 0.0,
            tokens_in: 0,
            tokens_out: 0,
            duration_ms: None,
            created_at: 0,
            started_at: None,
            error: None,
            review_of: None,
            cache_ratio: None,
        }
    }

    fn card(id: i64, outcome: RunOutcome) -> RunCard {
        RunCard {
            job: JobId(id),
            mission: None,
            title: "p".into(),
            outcome,
            evidence: None,
            cost_usd: 0.0,
            models: Vec::new(),
            labels: Vec::new(),
            at: 0,
            cache: None,
        }
    }

    fn set(h: &mut LedgerScreen, ids: &[i64]) {
        h.set_rows(
            ids.iter().map(|i| row(*i)).collect(),
            ids.iter().map(|i| card(*i, RunOutcome::Done)).collect(),
        );
    }

    #[test]
    fn moving_and_actions() {
        let mut h = LedgerScreen::default();
        assert_eq!(h.handle(Command::Rerun), LedgerAction::None);
        assert_eq!(h.handle(Command::Search), LedgerAction::Search);
        assert_eq!(h.handle(Command::Labels), LedgerAction::Labels);
        set(&mut h, &[3, 2]);
        assert_eq!(h.handle(Command::Prev), LedgerAction::None);
        assert_eq!(h.handle(Command::Next), LedgerAction::Moved);
        assert_eq!(h.handle(Command::Next), LedgerAction::None);
        assert_eq!(h.handle(Command::Rerun), LedgerAction::Rerun(JobId(2)));
        assert_eq!(h.handle(Command::Fork), LedgerAction::Fork(JobId(2)));
        assert_eq!(h.handle(Command::View), LedgerAction::View(JobId(2)));
        assert_eq!(
            h.handle(Command::Verify),
            LedgerAction::None,
            "no bundle, nothing to verify"
        );
    }

    #[test]
    fn new_rows_keep_the_selected_job() {
        let mut h = LedgerScreen::default();
        set(&mut h, &[2, 1]);
        h.selected = 1;
        set(&mut h, &[3, 2, 1]);
        assert_eq!(h.selected_row().unwrap().job_id, JobId(1));
        set(&mut h, &[9]);
        assert_eq!(h.selected, 0);
    }

    #[test]
    fn the_filter_cycles_and_admits() {
        let mut h = LedgerScreen::default();
        assert_eq!(h.handle(Command::Filter), LedgerAction::Filtered);
        assert_eq!(h.filter, Filter::Delivered);
        let mut seen = vec![h.filter];
        for _ in 0..3 {
            h.handle(Command::Filter);
            seen.push(h.filter);
        }
        assert_eq!(
            seen,
            [
                Filter::Delivered,
                Filter::NeedsYou,
                Filter::Labelled,
                Filter::All
            ]
        );
        assert!(Filter::All.admits(&card(1, RunOutcome::Done)));
        assert!(Filter::NeedsYou.admits(&card(1, RunOutcome::Escalated)));
        assert!(!Filter::NeedsYou.admits(&card(1, RunOutcome::Delivered)));
        assert!(!Filter::Labelled.admits(&card(1, RunOutcome::Delivered)));
    }
}
