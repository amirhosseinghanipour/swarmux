//! Focus: one agent in detail. The state lives here (scroll position, what
//! the user did, where they last looked); `model` gathers what the screen
//! shows and `ui::focus` draws it.

mod model;
mod sat;
pub mod stream;

use std::cell::Cell;
use std::collections::HashMap;

use swarmux_domain::{JobId, Millis};

pub use model::{Checkpoints, Focus, LaneKind, ReviewColumn};
pub use sat::Confidence;

/// Human events kept per job.
const HUMAN_KEPT: usize = 200;
/// How long the sigil of the job just opened is highlighted, in ms.
pub const ARRIVAL_MS: u64 = 300;

/// What the Focus screen remembers.
#[derive(Default)]
pub struct FocusState {
    /// Rows above the newest the stream is scrolled back; 0 follows the tail.
    pub scroll: usize,
    /// The furthest back the stream can go, as drawn last.
    pub max_scroll: Cell<usize>,
    /// Rows the answer (or diff) pane is scrolled down.
    pub answer_scroll: usize,
    pub max_answer_scroll: Cell<usize>,
    /// The working-tree diff shown in place of the answer, when asked for.
    pub diff: Option<Vec<String>>,
    /// The checkpoint `[` and `]` stopped on.
    pub checkpoint: Option<usize>,
    /// When the user prompted or answered, per job.
    human: HashMap<JobId, Vec<Millis>>,
    /// Stream rows the user had seen when they last looked, per job.
    seen: HashMap<JobId, usize>,
    /// The job on screen at the last update, and the rows already seen then.
    shown: Option<JobId>,
    marker: Option<(JobId, usize)>,
    /// The frame the current job was opened at.
    pub arrived_at: u64,
}

impl FocusState {
    /// The user did something to `job` at `at`.
    pub fn note_human(&mut self, job: JobId, at: Millis) {
        let events = self.human.entry(job).or_default();
        events.push(at);
        if events.len() > HUMAN_KEPT {
            events.remove(0);
        }
    }

    pub fn human_events(&self, job: JobId) -> &[Millis] {
        self.human.get(&job).map_or(&[], Vec::as_slice)
    }

    /// The row count to put the "you were here" line after, if the user
    /// has been away from `job` while it moved on.
    pub fn marker_for(&self, job: JobId) -> Option<usize> {
        self.marker.filter(|(j, _)| *j == job).map(|(_, n)| n)
    }

    /// Called once per update. `on_screen` is the job Focus is showing now
    /// (`None` on any other screen); `rows` is how many rows its stream has.
    pub fn observe(&mut self, on_screen: Option<(JobId, usize)>, frame: u64) {
        let Some((job, rows)) = on_screen else {
            self.shown = None;
            return;
        };
        if self.shown != Some(job) {
            // Just arrived: what was seen last time marks the spot.
            self.shown = Some(job);
            self.marker = self
                .seen
                .get(&job)
                .copied()
                .filter(|n| *n < rows)
                .map(|n| (job, n));
            self.scroll = 0;
            self.answer_scroll = 0;
            self.diff = None;
            self.checkpoint = None;
            self.arrived_at = frame;
        }
        self.seen.insert(job, rows);
    }

    pub fn scroll_by(&mut self, delta: isize) {
        self.scroll = self
            .scroll
            .saturating_add_signed(delta)
            .min(self.max_scroll.get());
    }

    pub fn scroll_answer(&mut self, delta: isize) {
        self.answer_scroll = self
            .answer_scroll
            .saturating_add_signed(delta)
            .min(self.max_answer_scroll.get());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_marker_sits_where_the_user_left() {
        let mut f = FocusState::default();
        f.observe(Some((JobId(1), 10)), 0);
        assert_eq!(f.marker_for(JobId(1)), None, "first visit: nothing is new");
        f.observe(Some((JobId(1), 12)), 1);
        f.observe(None, 2);
        f.observe(Some((JobId(1), 20)), 3);
        assert_eq!(f.marker_for(JobId(1)), Some(12));
        assert_eq!(f.marker_for(JobId(2)), None);
        // Staying keeps the spot while the stream grows.
        f.observe(Some((JobId(1), 25)), 4);
        assert_eq!(f.marker_for(JobId(1)), Some(12));
    }

    #[test]
    fn no_marker_when_nothing_happened_meanwhile() {
        let mut f = FocusState::default();
        f.observe(Some((JobId(1), 10)), 0);
        f.observe(None, 1);
        f.observe(Some((JobId(1), 10)), 2);
        assert_eq!(f.marker_for(JobId(1)), None);
    }

    #[test]
    fn human_events_are_kept_per_job_and_capped() {
        let mut f = FocusState::default();
        for i in 0..250 {
            f.note_human(JobId(1), i);
        }
        assert_eq!(f.human_events(JobId(1)).len(), HUMAN_KEPT);
        assert!(f.human_events(JobId(2)).is_empty());
    }

    #[test]
    fn scrolling_never_goes_below_the_tail() {
        let mut f = FocusState::default();
        f.max_scroll.set(10);
        f.scroll_by(-3);
        assert_eq!(f.scroll, 0);
        f.scroll_by(5);
        f.scroll_by(-2);
        assert_eq!(f.scroll, 3);
    }
}
