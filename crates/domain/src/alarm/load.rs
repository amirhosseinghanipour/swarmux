//! The supervisor-load gauge: the share of the last while the user spent
//! working the inbox. Above [`HOLD_AT`] new work waits.

use std::collections::VecDeque;

use crate::Millis;

/// The trailing window the share is taken over.
pub const LOAD_WINDOW_MS: Millis = 15 * 60_000;
/// Above this share the queue stops starting new work.
pub const HOLD_AT: f64 = 0.70;
/// Less than this much observed time says nothing yet.
const MIN_COVERAGE_MS: Millis = 60_000;

/// Time spent in the inbox, as stretches of engaged or free time.
#[derive(Clone, Debug)]
pub struct LoadMeter {
    window: Millis,
    /// `(start, end, engaged)`, oldest first, contiguous or not.
    spans: VecDeque<(Millis, Millis, bool)>,
    last: Option<(Millis, bool)>,
}

impl Default for LoadMeter {
    fn default() -> LoadMeter {
        LoadMeter::new(LOAD_WINDOW_MS)
    }
}

impl LoadMeter {
    pub fn new(window: Millis) -> LoadMeter {
        LoadMeter {
            window: window.max(1),
            spans: VecDeque::new(),
            last: None,
        }
    }

    /// Notes that from the previous call until `now` the user was in the
    /// state the previous call gave, and is now in `engaged`.
    pub fn record(&mut self, now: Millis, engaged: bool) {
        if let Some((from, was)) = self.last
            && now > from
        {
            match self.spans.back_mut() {
                Some((_, end, state)) if *end == from && *state == was => *end = now,
                _ => self.spans.push_back((from, now, was)),
            }
        }
        if self.last.is_none_or(|(from, _)| now >= from) {
            self.last = Some((now, engaged));
        }
        while self
            .spans
            .front()
            .is_some_and(|(_, end, _)| *end <= now - self.window)
        {
            self.spans.pop_front();
        }
    }

    /// The engaged share of the window ending `now`, `0.0..=1.0`; `None`
    /// until a minute has been watched.
    pub fn share(&self, now: Millis) -> Option<f64> {
        let from = now - self.window;
        let mut engaged: Millis = 0;
        let mut covered: Millis = 0;
        for (start, end, state) in &self.spans {
            let len = (*end).min(now) - (*start).max(from);
            if len > 0 {
                covered += len;
                if *state {
                    engaged += len;
                }
            }
        }
        (covered >= MIN_COVERAGE_MS).then(|| engaged as f64 / covered as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Millis = 60_000;

    #[test]
    fn nothing_is_said_before_a_minute_has_been_watched() {
        let mut m = LoadMeter::default();
        m.record(0, true);
        m.record(30_000, true);
        assert_eq!(m.share(30_000), None);
    }

    #[test]
    fn the_share_is_engaged_time_over_watched_time() {
        let mut m = LoadMeter::default();
        m.record(0, true);
        m.record(3 * MIN, false);
        m.record(4 * MIN, false);
        assert_eq!(m.share(4 * MIN), Some(0.75));
        let mut busy = LoadMeter::default();
        busy.record(0, true);
        busy.record(10 * MIN, true);
        assert_eq!(busy.share(10 * MIN), Some(1.0));
    }

    #[test]
    fn old_time_falls_out_of_the_window() {
        let mut m = LoadMeter::new(10 * MIN);
        m.record(0, true);
        m.record(5 * MIN, false);
        m.record(30 * MIN, false);
        assert_eq!(m.share(30 * MIN), Some(0.0));
    }

    #[test]
    fn a_clock_that_goes_back_is_ignored() {
        let mut m = LoadMeter::default();
        m.record(10 * MIN, true);
        m.record(5 * MIN, false);
        m.record(12 * MIN, true);
        assert!(m.share(12 * MIN).is_some());
    }
}
