//! Five-hour billing blocks and the burn rate inside them.
//!
//! A block starts at the hour of its first message and lasts five hours; a
//! message after the block ends, or after five idle hours, starts a new one
//! (the rule Claude's usage windows and `ccusage` use).

use super::pricing::TokenUsage;
use super::time::{HOUR, MINUTE};
use crate::Millis;

pub const BLOCK_LEN: Millis = 5 * HOUR;

/// One priced message for block and burn math.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UsagePoint {
    pub ts: Millis,
    /// 0 for a model without a known price.
    pub cost: f64,
    pub usage: TokenUsage,
}

/// Spend per unit of time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BurnRate {
    pub usd_per_hour: f64,
    pub tokens_per_minute: f64,
}

/// A five-hour billing block.
#[derive(Clone, Debug, PartialEq)]
pub struct BillingBlock {
    pub start: Millis,
    /// `start` + 5 h.
    pub end: Millis,
    pub first_ts: Millis,
    pub last_ts: Millis,
    pub cost: f64,
    pub tokens: TokenUsage,
    pub messages: u64,
    /// `now` is inside the block and the last message is less than 5 h old.
    pub active: bool,
    /// From the first to the latest message; `None` under a minute of activity.
    pub burn: Option<BurnRate>,
    /// Active blocks: the cost at the block's end if the burn rate holds.
    pub projected_cost: Option<f64>,
}

/// Blocks of `points` (sorted by time), oldest first.
pub fn billing_blocks(points: &[UsagePoint], now: Millis) -> Vec<BillingBlock> {
    let mut blocks: Vec<BillingBlock> = Vec::new();
    for p in points {
        let fits = blocks
            .last()
            .is_some_and(|b| p.ts < b.end && p.ts - b.last_ts < BLOCK_LEN);
        if !fits {
            let start = p.ts.div_euclid(HOUR) * HOUR;
            blocks.push(BillingBlock {
                start,
                end: start + BLOCK_LEN,
                first_ts: p.ts,
                last_ts: p.ts,
                cost: 0.0,
                tokens: TokenUsage::default(),
                messages: 0,
                active: false,
                burn: None,
                projected_cost: None,
            });
        }
        if let Some(b) = blocks.last_mut() {
            b.last_ts = b.last_ts.max(p.ts);
            b.cost += p.cost;
            b.tokens.add(&p.usage);
            b.messages += 1;
        }
    }
    for b in &mut blocks {
        b.active = now >= b.start && now < b.end && now - b.last_ts < BLOCK_LEN;
        b.burn = burn_rate(b);
        if b.active {
            b.projected_cost = b.burn.map(|r| {
                let left_h = (b.end - now) as f64 / HOUR as f64;
                b.cost + r.usd_per_hour * left_h
            });
        }
    }
    blocks
}

fn burn_rate(b: &BillingBlock) -> Option<BurnRate> {
    let span = b.last_ts - b.first_ts;
    (span >= MINUTE).then(|| BurnRate {
        usd_per_hour: b.cost / (span as f64 / HOUR as f64),
        tokens_per_minute: b.tokens.total() as f64 / (span as f64 / MINUTE as f64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(ts: Millis, cost: f64, output: u64) -> UsagePoint {
        UsagePoint {
            ts,
            cost,
            usage: TokenUsage {
                output,
                ..TokenUsage::default()
            },
        }
    }

    #[test]
    fn blocks_start_on_the_hour_and_last_five_hours() {
        let t = 100 * HOUR;
        let pts = [
            p(t + 20 * MINUTE, 1.0, 10),
            p(t + 4 * HOUR, 1.0, 10),
            // Past start + 5 h: a new block, starting at its own hour.
            p(t + 5 * HOUR + MINUTE, 2.0, 10),
        ];
        let b = billing_blocks(&pts, t + 5 * HOUR + 2 * MINUTE);
        assert_eq!(b.len(), 2);
        assert_eq!((b[0].start, b[0].end, b[0].messages), (t, t + BLOCK_LEN, 2));
        assert_eq!(b[1].start, t + 5 * HOUR);
        assert!(!b[0].active && b[1].active);
    }

    #[test]
    fn a_long_gap_starts_a_new_block() {
        let t = 10 * HOUR;
        let pts = [p(t, 1.0, 1), p(t + 6 * HOUR, 1.0, 1)];
        assert_eq!(billing_blocks(&pts, t + 7 * HOUR).len(), 2);
    }

    #[test]
    fn burn_rate_and_projection() {
        let t = 50 * HOUR;
        let pts = [p(t, 1.0, 600), p(t + 30 * MINUTE, 2.0, 600)];
        let now = t + HOUR;
        let b = &billing_blocks(&pts, now)[0];
        let burn = b.burn.unwrap();
        // $3 over half an hour, 1200 tokens over 30 minutes.
        assert!((burn.usd_per_hour - 6.0).abs() < 1e-9);
        assert!((burn.tokens_per_minute - 40.0).abs() < 1e-9);
        // 4 hours left at $6/h.
        assert!((b.projected_cost.unwrap() - 27.0).abs() < 1e-9);
    }

    #[test]
    fn a_single_message_has_no_burn_rate() {
        let b = &billing_blocks(&[p(0, 1.0, 1)], 1)[0];
        assert_eq!((b.burn, b.projected_cost), (None, None));
        assert!(b.active);
        assert!(billing_blocks(&[], 0).is_empty());
    }
}
