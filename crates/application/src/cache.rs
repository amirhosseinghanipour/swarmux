//! The prompt-cache metric: how much of what the agents read came from the
//! cache instead of being paid for in full.
//!
//! Caching cuts the cost of long agent sessions by half or more, and a
//! change that quietly breaks it (a prompt that moves, a tool list that
//! shuffles) shows up here first. The numbers are the ones the stream-json
//! usage reports per job; a job that reported none is unknown, not zero.

use std::collections::BTreeMap;

use swarmux_domain::{Millis, cache_read_ratio};

use crate::AppResult;

const DAY_MS: Millis = 24 * 60 * 60 * 1000;

/// One finished job's token usage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheRow {
    pub model: String,
    /// Input tokens that were not cached.
    pub tokens_in: u64,
    pub cache_read: u64,
    /// `None`: the job's usage did not say.
    pub cache_creation: Option<u64>,
}

/// Where the rows come from.
pub trait CacheSource {
    /// The finished jobs created at or after `since`.
    fn cache_rows(&self, since: Millis) -> AppResult<Vec<CacheRow>>;
}

/// Cache use over a set of jobs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheTotals {
    /// Jobs in the window.
    pub jobs: usize,
    /// Of those, jobs whose usage allows a ratio.
    pub known: usize,
    pub input: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
}

impl CacheTotals {
    fn add(&mut self, row: &CacheRow) {
        self.jobs += 1;
        if let Some(created) = row.cache_creation {
            self.known += 1;
            self.input += row.tokens_in;
            self.cache_read += row.cache_read;
            self.cache_creation += created;
        }
    }

    /// Share of input read from the cache; `None` when no job says.
    pub fn ratio(&self) -> Option<f64> {
        cache_read_ratio(self.input, self.cache_read, Some(self.cache_creation))
    }
}

/// The metric for the last `days` days.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CacheReport {
    pub days: i64,
    pub all: CacheTotals,
    /// Per model, most jobs first.
    pub models: Vec<(String, CacheTotals)>,
}

/// Cache use over the jobs of the last `days` days.
pub fn cache_report(source: &dyn CacheSource, now: Millis, days: i64) -> AppResult<CacheReport> {
    let days = days.max(1);
    let rows = source.cache_rows(now - days * DAY_MS)?;
    let mut all = CacheTotals::default();
    let mut by_model: BTreeMap<String, CacheTotals> = BTreeMap::new();
    for row in &rows {
        all.add(row);
        by_model.entry(row.model.clone()).or_default().add(row);
    }
    let mut models: Vec<_> = by_model.into_iter().collect();
    models.sort_by(|a, b| b.1.jobs.cmp(&a.1.jobs).then_with(|| a.0.cmp(&b.0)));
    Ok(CacheReport { days, all, models })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rows(Vec<CacheRow>);

    impl CacheSource for Rows {
        fn cache_rows(&self, _since: Millis) -> AppResult<Vec<CacheRow>> {
            Ok(self.0.clone())
        }
    }

    fn row(model: &str, input: u64, read: u64, created: Option<u64>) -> CacheRow {
        CacheRow {
            model: model.into(),
            tokens_in: input,
            cache_read: read,
            cache_creation: created,
        }
    }

    #[test]
    fn the_ratio_is_reads_over_everything_read_and_unknown_jobs_do_not_count() {
        let source = Rows(vec![
            row("opus", 100, 800, Some(100)),
            row("opus", 100, 200, Some(700)),
            row("sonnet", 50, 0, None),
        ]);
        let r = cache_report(&source, 10 * DAY_MS, 7).unwrap();
        assert_eq!((r.all.jobs, r.all.known), (3, 2));
        // (800 + 200) / (1000 + 1000)
        assert_eq!(r.all.ratio(), Some(0.5));
        assert_eq!(r.models[0].0, "opus");
        assert_eq!(r.models[0].1.ratio(), Some(0.5));
        assert_eq!(
            r.models[1].1.ratio(),
            None,
            "sonnet reported no cache usage"
        );
    }

    #[test]
    fn nothing_known_is_not_zero() {
        let r = cache_report(&Rows(Vec::new()), 0, 7).unwrap();
        assert_eq!(r.all.ratio(), None);
        let r = cache_report(&Rows(vec![row("m", 5, 0, None)]), 0, 7).unwrap();
        assert_eq!(r.all.ratio(), None);
    }
}
