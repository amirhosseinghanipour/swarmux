//! State of the history screen: the rows, the search and the selected job's detail.

use crossterm::event::KeyCode;
use swarmux_application::{JobDetail, JobSummary};
use swarmux_domain::JobId;

/// What a key on the history screen asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryAction {
    None,
    /// The selection moved; load its detail.
    Moved,
    Search,
    Rerun(JobId),
    Fork(JobId),
    View(JobId),
}

#[derive(Default)]
pub struct HistoryScreen {
    pub rows: Vec<JobSummary>,
    pub selected: usize,
    pub query: String,
    pub detail: Option<JobDetail>,
}

impl HistoryScreen {
    pub fn selected_row(&self) -> Option<&JobSummary> {
        self.rows.get(self.selected)
    }

    /// Replaces the rows, keeping the selected job selected if it is still there.
    pub fn set_rows(&mut self, rows: Vec<JobSummary>) {
        let keep = self.selected_row().map(|r| r.job_id);
        self.rows = rows;
        if let Some(i) = keep.and_then(|id| self.rows.iter().position(|r| r.job_id == id)) {
            self.selected = i;
        }
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    pub fn handle(&mut self, key: KeyCode) -> HistoryAction {
        if key == KeyCode::Char('/') {
            return HistoryAction::Search;
        }
        let Some(id) = self.selected_row().map(|r| r.job_id) else {
            return HistoryAction::None;
        };
        match key {
            KeyCode::Up if self.selected > 0 => {
                self.selected -= 1;
                HistoryAction::Moved
            }
            KeyCode::Down if self.selected + 1 < self.rows.len() => {
                self.selected += 1;
                HistoryAction::Moved
            }
            KeyCode::Char('r') => HistoryAction::Rerun(id),
            KeyCode::Char('f') => HistoryAction::Fork(id),
            KeyCode::Enter => HistoryAction::View(id),
            _ => HistoryAction::None,
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
        }
    }

    #[test]
    fn moving_and_actions() {
        let mut h = HistoryScreen::default();
        assert_eq!(h.handle(KeyCode::Char('r')), HistoryAction::None);
        assert_eq!(h.handle(KeyCode::Char('/')), HistoryAction::Search);
        h.set_rows(vec![row(3), row(2)]);
        assert_eq!(h.handle(KeyCode::Up), HistoryAction::None);
        assert_eq!(h.handle(KeyCode::Down), HistoryAction::Moved);
        assert_eq!(h.handle(KeyCode::Down), HistoryAction::None);
        assert_eq!(h.handle(KeyCode::Char('r')), HistoryAction::Rerun(JobId(2)));
        assert_eq!(h.handle(KeyCode::Char('f')), HistoryAction::Fork(JobId(2)));
        assert_eq!(h.handle(KeyCode::Enter), HistoryAction::View(JobId(2)));
    }

    #[test]
    fn new_rows_keep_the_selected_job() {
        let mut h = HistoryScreen::default();
        h.set_rows(vec![row(2), row(1)]);
        h.selected = 1;
        h.set_rows(vec![row(3), row(2), row(1)]);
        assert_eq!(h.selected_row().unwrap().job_id, JobId(1));
        h.set_rows(vec![row(9)]);
        assert_eq!(h.selected, 0);
    }
}
