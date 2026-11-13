//! State of the queue screen: which job is selected.

use swarmux_application::QueueEntry;
use swarmux_domain::JobId;

/// What the user asks of the selected job (or the selection).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Prev,
    Next,
    Cancel,
    Retry,
    PriorityUp,
    PriorityDown,
    View,
}

/// What a command on the queue screen comes to.
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

    pub fn handle(&mut self, command: Command, rows: &[QueueEntry]) -> QueueAction {
        let Some(current) = rows.get(self.index(rows)) else {
            return QueueAction::None;
        };
        let i = self.index(rows);
        let id = current.job_id;
        match command {
            Command::Prev => self.selected = Some(rows[i.saturating_sub(1)].job_id),
            Command::Next => self.selected = Some(rows[(i + 1).min(rows.len() - 1)].job_id),
            Command::Cancel => return QueueAction::Cancel(id),
            Command::Retry => return QueueAction::Retry(id),
            Command::PriorityUp => return QueueAction::Priority(id, 1),
            Command::PriorityDown => return QueueAction::Priority(id, -1),
            Command::View => return QueueAction::View(id),
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
                note: None,
            })
            .collect()
    }

    #[test]
    fn selection_follows_the_job() {
        let mut q = QueueScreen::default();
        let r = rows(&[1, 2, 3]);
        q.handle(Command::Next, &r);
        q.handle(Command::Next, &r);
        q.handle(Command::Next, &r);
        assert_eq!(q.selected, Some(JobId(3)));
        assert_eq!(q.index(&rows(&[3, 1])), 0);
        assert_eq!(q.index(&rows(&[1, 2])), 0, "gone job falls back to the top");
        q.handle(Command::Prev, &rows(&[1, 3]));
        assert_eq!(q.selected, Some(JobId(1)));
    }

    #[test]
    fn keys_map_to_actions_on_the_selected_job() {
        let mut q = QueueScreen::default();
        let r = rows(&[4, 5]);
        q.handle(Command::Next, &r);
        assert_eq!(q.handle(Command::Cancel, &r), QueueAction::Cancel(JobId(5)));
        assert_eq!(
            q.handle(Command::PriorityUp, &r),
            QueueAction::Priority(JobId(5), 1)
        );
        assert_eq!(
            q.handle(Command::PriorityDown, &r),
            QueueAction::Priority(JobId(5), -1)
        );
        assert_eq!(q.handle(Command::Retry, &r), QueueAction::Retry(JobId(5)));
        assert_eq!(q.handle(Command::View, &r), QueueAction::View(JobId(5)));
        assert_eq!(q.handle(Command::Cancel, &[]), QueueAction::None);
    }
}
