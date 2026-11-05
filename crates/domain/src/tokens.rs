//! Token counts per actor, summed from usage reports.

/// A rough token count of `text`: one token per four characters, rounded up.
/// Good enough to keep injected text under a budget; never used for billing.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// Usage of one actor (the lead, a subagent or the reviewer).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TokenTally {
    /// Every token the actor read or wrote, summed over its turns.
    pub seen: u64,
    pub output: u64,
    pub cache_read: u64,
    /// Context of its latest turn (input + cache read + cache creation).
    pub context: u64,
    pub turns: u32,
    /// Ticks of the first and latest usage report.
    pub first_tick: Option<u32>,
    pub last_tick: u32,
    /// The latest report, to skip the copies Claude Code repeats on each
    /// content block of one message.
    last: Option<[u64; 4]>,
}

impl TokenTally {
    /// Adds one usage report. A report equal to the previous one is the same
    /// message repeated and is not counted again.
    pub fn add(
        &mut self,
        input: u64,
        output: u64,
        cache_read: u64,
        cache_creation: u64,
        tick: u32,
    ) {
        let key = [input, output, cache_read, cache_creation];
        if self.last == Some(key) {
            return;
        }
        self.last = Some(key);
        let context = input + cache_read + cache_creation;
        self.context = context;
        self.seen += context + output;
        self.output += output;
        self.cache_read += cache_read;
        self.turns += 1;
        self.first_tick.get_or_insert(tick);
        self.last_tick = self.last_tick.max(tick);
    }

    /// Output tokens per second, once the actor has reported for at least a second.
    pub fn rate(&self, since: Option<u32>) -> Option<f64> {
        let start = since.or(self.first_tick)?;
        let secs = f64::from(self.last_tick.saturating_sub(start)) / 10.0;
        (secs >= 1.0 && self.output > 0).then(|| self.output as f64 / secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_reports_count_once() {
        let mut t = TokenTally::default();
        t.add(2, 10, 100, 5, 1);
        t.add(2, 10, 100, 5, 2);
        t.add(3, 20, 150, 0, 30);
        assert_eq!(t.turns, 2);
        assert_eq!(t.seen, 117 + 173);
        assert_eq!((t.output, t.cache_read, t.context), (30, 250, 153));
        assert_eq!((t.first_tick, t.last_tick), (Some(1), 30));
    }

    #[test]
    fn rate_needs_a_second() {
        let mut t = TokenTally::default();
        assert_eq!(t.rate(None), None);
        t.add(1, 50, 0, 0, 5);
        assert_eq!(t.rate(None), None);
        t.add(1, 50, 1, 0, 25);
        assert_eq!(t.rate(None), Some(50.0));
        assert_eq!(t.rate(Some(0)), Some(40.0));
    }
}
