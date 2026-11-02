//! State of the queue screen: which job is selected.

use crossterm::event::KeyCode;
use swarmux_application::QueueEntry;
use swarmux_domain::JobId;

/// What a key on the queue screen asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueAction {
    None,
    Cancel(JobId),
    Retry(JobId),
    Priority(JobId, i32),
    View(JobId),
}

/// Selection is kept by job, so it stays put while rows move.
#[derive(Default)]
pub struct QueueScreen {
    pub selected: Option<JobId>,
}

impl QueueScreen {
    /// Row of the selected job (the first row if it is gone).
    pub fn index(&self, rows: &[QueueEntry]) -> usize {
        self.selected
            .and_then(|id| rows.iter().position(|r| r.job_id == id))
            .unwrap_or(0)
    }

    pub fn handle(&mut self, key: KeyCode, rows: &[QueueEntry]) -> QueueAction {
        let Some(current) = rows.get(self.index(rows)) else {
            return QueueAction::None;
        };
        let i = self.index(rows);
        let id = current.job_id;
        match key {
            KeyCode::Up => self.selected = Some(rows[i.saturating_sub(1)].job_id),
            KeyCode::Down => self.selected = Some(rows[(i + 1).min(rows.len() - 1)].job_id),
            KeyCode::Char('c') => return QueueAction::Cancel(id),
            KeyCode::Char('r') => return QueueAction::Retry(id),
            KeyCode::Char('+' | '=') => return QueueAction::Priority(id, 1),
            KeyCode::Char('-') => return QueueAction::Priority(id, -1),
            KeyCode::Enter => return QueueAction::View(id),
            _ => {}
        }
        QueueAction::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_domain::JobStatus;

    fn rows(ids: &[i64]) -> Vec<QueueEntry> {
        ids.iter()
            .map(|&i| QueueEntry {
                job_id: JobId(i),
                session_name: "s".into(),
                prompt: "p".into(),
                model: "m".into(),
                status: JobStatus::Queued,
                priority: 0,
                cost_usd: 0.0,
            })
            .collect()
    }

    #[test]
    fn selection_follows_the_job() {
        let mut q = QueueScreen::default();
        let r = rows(&[1, 2, 3]);
        q.handle(KeyCode::Down, &r);
        q.handle(KeyCode::Down, &r);
        q.handle(KeyCode::Down, &r);
        assert_eq!(q.selected, Some(JobId(3)));
        assert_eq!(q.index(&rows(&[3, 1])), 0);
        assert_eq!(q.index(&rows(&[1, 2])), 0, "gone job falls back to the top");
        q.handle(KeyCode::Up, &rows(&[1, 3]));
        assert_eq!(q.selected, Some(JobId(1)));
    }

    #[test]
    fn keys_map_to_actions_on_the_selected_job() {
        let mut q = QueueScreen::default();
        let r = rows(&[4, 5]);
        q.handle(KeyCode::Down, &r);
        assert_eq!(
            q.handle(KeyCode::Char('c'), &r),
            QueueAction::Cancel(JobId(5))
        );
        assert_eq!(
            q.handle(KeyCode::Char('+'), &r),
            QueueAction::Priority(JobId(5), 1)
        );
        assert_eq!(
            q.handle(KeyCode::Char('-'), &r),
            QueueAction::Priority(JobId(5), -1)
        );
        assert_eq!(
            q.handle(KeyCode::Char('r'), &r),
            QueueAction::Retry(JobId(5))
        );
        assert_eq!(q.handle(KeyCode::Enter, &r), QueueAction::View(JobId(5)));
        assert_eq!(q.handle(KeyCode::Char('c'), &[]), QueueAction::None);
    }
}
