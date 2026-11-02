//! Token flow: tokens per minute over the last hour.

use std::collections::VecDeque;

use super::pricing::TokenUsage;
use super::time::MINUTE;
use crate::Millis;

/// Minutes a flow series covers.
pub const FLOW_MINUTES: usize = 60;

/// Tokens reported during one minute.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlowBucket {
    /// Minutes since the epoch (UTC).
    pub minute: i64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl FlowBucket {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// A rolling per-minute tally of the last [`FLOW_MINUTES`] minutes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TokenFlow {
    buckets: VecDeque<FlowBucket>,
}

impl TokenFlow {
    /// Adds tokens reported at `ts`. Reports older than the window are dropped.
    pub fn add(&mut self, ts: Millis, usage: &TokenUsage) {
        let minute = ts.div_euclid(MINUTE);
        let bucket = match self.buckets.iter_mut().rev().find(|b| b.minute == minute) {
            Some(b) => b,
            None => {
                let at = self.buckets.partition_point(|b| b.minute < minute);
                self.buckets.insert(
                    at,
                    FlowBucket {
                        minute,
                        ..FlowBucket::default()
                    },
                );
                &mut self.buckets[at]
            }
        };
        bucket.input += usage.input;
        bucket.output += usage.output;
        bucket.cache_read += usage.cache_read;
        bucket.cache_write += usage.cache_write();
        let newest = self.buckets.back().map_or(minute, |b| b.minute);
        while self
            .buckets
            .front()
            .is_some_and(|b| b.minute <= newest - FLOW_MINUTES as i64)
        {
            self.buckets.pop_front();
        }
    }

    /// Exactly [`FLOW_MINUTES`] buckets, oldest first, ending with `now`'s minute.
    pub fn series(&self, now: Millis) -> Vec<FlowBucket> {
        series_of(self.buckets.iter().copied(), now)
    }

    /// Cache reads over every prompt token in the last hour.
    pub fn cache_hit_ratio(&self, now: Millis) -> Option<f64> {
        cache_hit_ratio(&self.series(now))
    }
}

/// Buckets (any order, may repeat minutes) as a dense series ending at `now`.
pub fn series_of(buckets: impl Iterator<Item = FlowBucket>, now: Millis) -> Vec<FlowBucket> {
    let last = now.div_euclid(MINUTE);
    let first = last - FLOW_MINUTES as i64 + 1;
    let mut out: Vec<FlowBucket> = (first..=last)
        .map(|minute| FlowBucket {
            minute,
            ..FlowBucket::default()
        })
        .collect();
    for b in buckets.filter(|b| (first..=last).contains(&b.minute)) {
        let slot = &mut out[(b.minute - first) as usize];
        slot.input += b.input;
        slot.output += b.output;
        slot.cache_read += b.cache_read;
        slot.cache_write += b.cache_write;
    }
    out
}

/// Cache reads over input + cache reads + cache writes; `None` without prompt tokens.
pub fn cache_hit_ratio(series: &[FlowBucket]) -> Option<f64> {
    let read: u64 = series.iter().map(|b| b.cache_read).sum();
    let prompt: u64 = series
        .iter()
        .map(|b| b.input + b.cache_read + b.cache_write)
        .sum();
    (prompt > 0).then(|| read as f64 / prompt as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(input: u64, output: u64, read: u64, write: u64) -> TokenUsage {
        TokenUsage {
            input,
            output,
            cache_read: read,
            cache_write_5m: write,
            cache_write_1h: 0,
        }
    }

    #[test]
    fn buckets_by_minute_and_fills_gaps() {
        let mut f = TokenFlow::default();
        let t0 = 1_000 * MINUTE;
        f.add(t0 + 1_000, &u(1, 2, 3, 4));
        f.add(t0 + 59_000, &u(1, 0, 0, 0));
        f.add(t0 + 2 * MINUTE, &u(0, 5, 0, 0));
        let s = f.series(t0 + 2 * MINUTE + 10);
        assert_eq!(s.len(), FLOW_MINUTES);
        assert_eq!(s.last().map(|b| (b.minute, b.output)), Some((1002, 5)));
        assert_eq!(s[FLOW_MINUTES - 2].total(), 0);
        assert_eq!(
            (s[FLOW_MINUTES - 3].minute, s[FLOW_MINUTES - 3].input),
            (1000, 2)
        );
    }

    #[test]
    fn old_minutes_leave_the_window() {
        let mut f = TokenFlow::default();
        f.add(0, &u(7, 0, 0, 0));
        f.add(70 * MINUTE, &u(1, 0, 0, 0));
        assert_eq!(f.buckets.len(), 1);
        // A late report still lands in its own minute.
        f.add(69 * MINUTE, &u(2, 0, 0, 0));
        let s = f.series(70 * MINUTE);
        assert_eq!(s.iter().map(|b| b.input).sum::<u64>(), 3);
        assert_eq!(s[FLOW_MINUTES - 2].input, 2);
    }

    #[test]
    fn cache_ratio_over_the_hour() {
        let mut f = TokenFlow::default();
        assert_eq!(f.cache_hit_ratio(0), None);
        f.add(0, &u(10, 99, 60, 30));
        assert_eq!(f.cache_hit_ratio(0), Some(0.6));
    }
}
