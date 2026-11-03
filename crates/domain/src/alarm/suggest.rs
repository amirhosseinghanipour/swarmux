//! Turning a habit into a rule: when the same ask has been approved enough
//! times, without a single refusal, the user is offered a rule instead.

use super::risk::RiskTier;

/// Identical approvals, with no refusal, before a rule is offered.
pub const SUGGEST_AFTER: u32 = 10;

/// How a signature has been answered in one repository.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub approved: u32,
    pub rejected: u32,
    /// The user was offered a rule (and took it or turned it down).
    pub suggested: bool,
}

impl Tally {
    /// Whether to offer a rule now. Never for destructive calls.
    pub fn should_suggest(&self, tier: RiskTier) -> bool {
        tier != RiskTier::Destructive
            && !self.suggested
            && self.rejected == 0
            && self.approved >= SUGGEST_AFTER
    }

    pub fn answered(&self) -> u32 {
        self.approved + self.rejected
    }
}

/// How new a call pattern is: `1.0` never seen, falling to `0.0` once it
/// has been answered many times.
pub fn novelty(tally: &Tally) -> f64 {
    match tally.answered() {
        0 => 1.0,
        1..=2 => 0.6,
        3..=9 => 0.3,
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tally(approved: u32, rejected: u32, suggested: bool) -> Tally {
        Tally {
            approved,
            rejected,
            suggested,
        }
    }

    #[test]
    fn ten_clean_approvals_earn_an_offer() {
        let ok = |t: Tally| t.should_suggest(RiskTier::Routine);
        assert!(!ok(tally(9, 0, false)));
        assert!(ok(tally(10, 0, false)));
        assert!(ok(tally(37, 0, false)));
        assert!(!ok(tally(37, 1, false)), "one refusal ends it");
        assert!(!ok(tally(37, 0, true)), "asked once");
        assert!(!tally(37, 0, false).should_suggest(RiskTier::Destructive));
        assert!(tally(37, 0, false).should_suggest(RiskTier::Elevated));
    }

    #[test]
    fn novelty_fades_with_use() {
        assert_eq!(novelty(&Tally::default()), 1.0);
        assert!(novelty(&tally(2, 0, false)) > novelty(&tally(5, 0, false)));
        assert_eq!(novelty(&tally(30, 0, false)), 0.0);
    }
}
