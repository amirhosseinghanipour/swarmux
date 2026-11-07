//! The rubber-stamp guard: when the owner approves nearly everything of a
//! kind, the approvals have probably stopped being decisions. The brief
//! then asks for one spot check.

use super::FollowKind;

/// Decisions of one kind that count, newest first.
pub const RUBBER_STAMP_WINDOW: usize = 50;
/// Fewer decisions than this say nothing.
pub const RUBBER_STAMP_MIN: usize = 20;
/// Approval share from which the guard speaks.
pub const RUBBER_STAMP_RATE: f64 = 0.95;

/// A kind the owner approves almost always.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stamped {
    pub kind: FollowKind,
    pub approved: usize,
    pub total: usize,
}

impl Stamped {
    pub fn rate(&self) -> f64 {
        self.approved as f64 / self.total.max(1) as f64
    }
}

/// The kinds approved at [`RUBBER_STAMP_RATE`] or more over their last
/// [`RUBBER_STAMP_WINDOW`] owner decisions, given at least
/// [`RUBBER_STAMP_MIN`] of them. `decisions` are the owner's own (a
/// standing order is not a decision), newest first, as `(kind, approved)`.
/// Deferring is not a decision and is left out by the caller.
pub fn rubber_stamped(decisions: &[(FollowKind, bool)]) -> Vec<Stamped> {
    FollowKind::ALL
        .into_iter()
        .filter_map(|kind| {
            let window: Vec<bool> = decisions
                .iter()
                .filter(|(k, _)| *k == kind)
                .take(RUBBER_STAMP_WINDOW)
                .map(|(_, a)| *a)
                .collect();
            let approved = window.iter().filter(|a| **a).count();
            let s = Stamped {
                kind,
                approved,
                total: window.len(),
            };
            (s.total >= RUBBER_STAMP_MIN && s.rate() >= RUBBER_STAMP_RATE).then_some(s)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(kind: FollowKind, approved: usize, declined: usize) -> Vec<(FollowKind, bool)> {
        std::iter::repeat_n((kind, false), declined)
            .chain(std::iter::repeat_n((kind, true), approved))
            .collect()
    }

    #[test]
    fn twenty_approvals_in_a_row_are_a_rubber_stamp() {
        let s = rubber_stamped(&run(FollowKind::SuggestedTask, 20, 0));
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].kind, FollowKind::SuggestedTask);
        assert!((s[0].rate() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn too_few_decisions_say_nothing() {
        assert!(rubber_stamped(&run(FollowKind::Risk, 19, 0)).is_empty());
    }

    #[test]
    fn one_decline_in_twenty_is_exactly_the_threshold() {
        assert_eq!(rubber_stamped(&run(FollowKind::Decision, 19, 1)).len(), 1);
        assert!(rubber_stamped(&run(FollowKind::Decision, 18, 2)).is_empty());
    }

    #[test]
    fn only_the_window_counts_and_kinds_are_apart() {
        // Newest 50 approved; older declines fall out of the window.
        let mut d = run(FollowKind::SuggestedTask, 50, 0);
        d.extend(run(FollowKind::SuggestedTask, 0, 30));
        d.extend(run(FollowKind::Risk, 5, 20));
        let s = rubber_stamped(&d);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].total, RUBBER_STAMP_WINDOW);
    }
}
