//! Flake history: how often each check of each repository failed and then
//! passed when run again.
//!
//! The gate runs a failing check once more, and a pass the second time is
//! shown as flaky on that verdict. One such run says little; a check that
//! does it three times in ten says the check, not the change, is what to
//! look at. This keeps the last runs per check and names the ones over the
//! line, so the inbox can say "flaky" instead of blaming the agent.

use std::collections::{HashMap, VecDeque};

/// Runs of one check, newest last: whether each failed and then passed.
#[derive(Clone, Debug, Default)]
pub struct FlakeHistory {
    runs: HashMap<(String, String), VecDeque<bool>>,
}

impl FlakeHistory {
    /// Notes one run of `check` in the repository at `root`, keeping the
    /// last `window` runs.
    pub fn record(&mut self, root: &str, check: &str, flaked: bool, window: u32) {
        let runs = self
            .runs
            .entry((root.to_string(), check.to_string()))
            .or_default();
        runs.push_back(flaked);
        let keep = window.max(1) as usize;
        while runs.len() > keep {
            runs.pop_front();
        }
    }

    /// How many of the last `window` runs of `check` flaked and how many
    /// runs that is, when at least `after` did; `None` when `after` is 0
    /// (off) or the check is not over the line.
    pub fn flaky(&self, root: &str, check: &str, after: u32, window: u32) -> Option<(u32, u32)> {
        if after == 0 {
            return None;
        }
        let runs = self.runs.get(&(root.to_string(), check.to_string()))?;
        let last: Vec<&bool> = runs.iter().rev().take(window.max(1) as usize).collect();
        let flaked = u32::try_from(last.iter().filter(|f| ***f).count()).unwrap_or(u32::MAX);
        (flaked >= after).then(|| (flaked, u32::try_from(last.len()).unwrap_or(u32::MAX)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_that_flakes_three_times_in_ten_is_flaky_and_stops_being_when_it_settles() {
        let mut h = FlakeHistory::default();
        for i in 0..10 {
            h.record("/r", "unit", i % 4 == 0, 10);
        }
        assert_eq!(h.flaky("/r", "unit", 3, 10), Some((3, 10)));
        assert_eq!(h.flaky("/r", "unit", 4, 10), None);
        assert_eq!(h.flaky("/r", "other", 3, 10), None);
        assert_eq!(h.flaky("/elsewhere", "unit", 3, 10), None, "per repository");
        assert_eq!(h.flaky("/r", "unit", 0, 10), None, "0 turns it off");
        for _ in 0..10 {
            h.record("/r", "unit", false, 10);
        }
        assert_eq!(h.flaky("/r", "unit", 3, 10), None, "old flakes age out");
    }
}
