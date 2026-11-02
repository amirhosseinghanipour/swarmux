//! Claude subscription usage limits (`rate_limit_event`) and when the current
//! pace reaches them.

use super::time::{DAY, HOUR, MINUTE};
use crate::Millis;

/// One limit window as Claude Code reported it.
#[derive(Clone, Debug, PartialEq)]
pub struct RateLimit {
    /// `five_hour`, `seven_day`, `seven_day_opus`, …
    pub window: String,
    /// `allowed`, `allowed_warning`, `rejected`, …
    pub status: String,
    /// Share of the window used, 0 to 1.
    pub utilization: Option<f64>,
    pub resets_at: Option<Millis>,
    /// The warning threshold crossed, if any (0 to 1).
    pub surpassed_threshold: Option<f64>,
}

/// How long a window lasts, for the windows Claude Code names.
pub fn window_length(window: &str) -> Option<Millis> {
    match window {
        "five_hour" => Some(5 * HOUR),
        w if w.starts_with("seven_day") => Some(7 * DAY),
        _ => None,
    }
}

/// Where a projection's pace came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectionBasis {
    /// The change between recent readings.
    Recent,
    /// The average pace since the window opened.
    WindowAverage,
}

/// When the limit is reached if usage keeps its pace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LimitProjection {
    pub hits_at: Millis,
    pub basis: ProjectionBasis,
}

/// Readings closer together than this don't give a recent pace.
const MIN_RECENT_SPAN: Millis = 5 * MINUTE;
/// Readings older than this don't count as recent.
const RECENT: Millis = HOUR;

/// When utilization reaches 1 at the current pace, if before the reset.
/// `samples` are (time, utilization) readings of this window, oldest first.
/// Uses the pace of the last hour when it spans at least five minutes and
/// rises, else the average since the window opened (`resets_at - len`).
pub fn project_limit(
    samples: &[(Millis, f64)],
    resets_at: Millis,
    window_len: Option<Millis>,
    now: Millis,
) -> Option<LimitProjection> {
    let &(last_at, last) = samples.last()?;
    if last >= 1.0 {
        return Some(LimitProjection {
            hits_at: last_at.min(now),
            basis: ProjectionBasis::Recent,
        });
    }
    let recent = samples
        .iter()
        .find(|(at, _)| last_at - at <= RECENT)
        .filter(|(at, u)| last_at - at >= MIN_RECENT_SPAN && *u < last)
        .map(|&(at, u)| {
            let per_ms = (last - u) / (last_at - at) as f64;
            LimitProjection {
                hits_at: last_at + ((1.0 - last) / per_ms) as Millis,
                basis: ProjectionBasis::Recent,
            }
        });
    let average = || {
        let opened = resets_at - window_len?;
        let elapsed = last_at - opened;
        (last > 0.0 && elapsed > 0).then(|| LimitProjection {
            hits_at: opened + (elapsed as f64 / last) as Millis,
            basis: ProjectionBasis::WindowAverage,
        })
    };
    recent.or_else(average).filter(|p| p.hits_at < resets_at)
}

/// The latest reading of one window plus recent readings for its pace.
#[derive(Clone, Debug, PartialEq)]
pub struct LimitState {
    pub limit: RateLimit,
    pub observed_at: Millis,
    /// (time, utilization), oldest first, same reset time as `limit`.
    pub samples: Vec<(Millis, f64)>,
}

/// Readings kept per window.
const MAX_SAMPLES: usize = 120;

impl LimitState {
    pub fn new(limit: RateLimit, observed_at: Millis) -> LimitState {
        let samples = limit
            .utilization
            .map(|u| vec![(observed_at, u)])
            .unwrap_or_default();
        LimitState {
            limit,
            observed_at,
            samples,
        }
    }

    /// Takes a newer reading; a new reset time starts the samples over.
    pub fn update(&mut self, limit: RateLimit, at: Millis) {
        if limit.resets_at != self.limit.resets_at {
            self.samples.clear();
        }
        if let Some(u) = limit.utilization {
            self.samples.push((at, u));
            if self.samples.len() > MAX_SAMPLES {
                self.samples.remove(0);
            }
        }
        self.limit = limit;
        self.observed_at = at;
    }

    /// Time left until the window resets (0 once passed).
    pub fn resets_in(&self, now: Millis) -> Option<Millis> {
        self.limit.resets_at.map(|r| (r - now).max(0))
    }

    /// The reading is from a window that has already reset.
    pub fn is_stale(&self, now: Millis) -> bool {
        self.limit.resets_at.is_some_and(|r| r <= now)
    }

    pub fn projection(&self, now: Millis) -> Option<LimitProjection> {
        if self.is_stale(now) {
            return None;
        }
        project_limit(
            &self.samples,
            self.limit.resets_at?,
            window_length(&self.limit.window),
            now,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit(window: &str, u: f64, resets_at: Millis) -> RateLimit {
        RateLimit {
            window: window.into(),
            status: "allowed".into(),
            utilization: Some(u),
            resets_at: Some(resets_at),
            surpassed_threshold: None,
        }
    }

    #[test]
    fn window_lengths() {
        assert_eq!(window_length("five_hour"), Some(5 * HOUR));
        assert_eq!(window_length("seven_day_opus"), Some(7 * DAY));
        assert_eq!(window_length("monthly"), None);
    }

    #[test]
    fn average_pace_since_the_window_opened() {
        // Opened at 0, 2 h in and 50% used: 100% at 4 h, before the 5 h reset.
        let reset = 5 * HOUR;
        let p = project_limit(&[(2 * HOUR, 0.5)], reset, Some(5 * HOUR), 2 * HOUR).unwrap();
        assert_eq!(p.hits_at, 4 * HOUR);
        assert_eq!(p.basis, ProjectionBasis::WindowAverage);
        // 20% after 2 h: 100% at 10 h, after the reset, so never.
        assert_eq!(
            project_limit(&[(2 * HOUR, 0.2)], reset, Some(5 * HOUR), 2 * HOUR),
            None
        );
    }

    #[test]
    fn recent_pace_wins_when_it_spans_enough() {
        let reset = 7 * DAY;
        let s = [(DAY, 0.50), (DAY + 30 * MINUTE, 0.60)];
        // +10% per 30 min: 40% more takes 2 h.
        let p = project_limit(&s, reset, Some(7 * DAY), DAY + 30 * MINUTE).unwrap();
        assert_eq!(p.hits_at, DAY + 30 * MINUTE + 2 * HOUR);
        assert_eq!(p.basis, ProjectionBasis::Recent);
        // Two readings a minute apart fall back to the window average.
        let s = [(DAY, 0.50), (DAY + MINUTE, 0.60)];
        let p = project_limit(&s, reset, Some(7 * DAY), DAY + MINUTE);
        assert_eq!(p.map(|p| p.basis), Some(ProjectionBasis::WindowAverage));
    }

    #[test]
    fn at_the_limit_or_unknown() {
        let p = project_limit(&[(10, 1.0)], 100, None, 20).unwrap();
        assert_eq!(p.hits_at, 10);
        assert_eq!(project_limit(&[], 100, None, 20), None);
        // Unknown window length and no recent pace.
        assert_eq!(project_limit(&[(10, 0.5)], 100, None, 20), None);
        // Nothing used yet.
        assert_eq!(project_limit(&[(10, 0.0)], 100, Some(100), 20), None);
    }

    #[test]
    fn state_keeps_samples_until_the_reset_moves() {
        let mut s = LimitState::new(limit("five_hour", 0.1, 5 * HOUR), HOUR);
        s.update(limit("five_hour", 0.5, 5 * HOUR), 2 * HOUR);
        assert_eq!(s.samples.len(), 2);
        assert_eq!(s.resets_in(4 * HOUR), Some(HOUR));
        assert!(s.projection(2 * HOUR).is_some());
        s.update(limit("five_hour", 0.05, 10 * HOUR), 6 * HOUR);
        assert_eq!(s.samples, [(6 * HOUR, 0.05)]);
        assert!(s.is_stale(10 * HOUR));
        assert_eq!(s.projection(10 * HOUR), None);
        assert_eq!(s.resets_in(11 * HOUR), Some(0));
    }
}
