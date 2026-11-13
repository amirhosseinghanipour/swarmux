//! Who may resolve a held call, and the one rule about irreversible calls.
//!
//! A call that cannot be taken back (R4: a delete, a force push, a history
//! rewrite) is allowed by one thing only: a person typing the name of what
//! it acts on, for that very call. A rule, a model's answer, a timeout, a
//! key press that is not the typed confirmation, an "always allow" and a
//! client that was not shown the call all fall short of it. The
//! application layer asks [`may_resolve`] before it lets any held call end
//! in an allow, so a client other than the terminal cannot get around it.

use crate::Verdict;
use crate::alarm::{Reversibility, reversibility};

/// What resolved a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Resolver {
    /// A person named the target of the call after it armed.
    HumanTyped,
    /// A person pressed an ordinary approval key.
    HumanKey,
    /// A configured, learned or built-in rule.
    Rule,
    /// An automated decider. None ships: the hosted JEV model was one, and
    /// was removed. Kept so a later extension cannot allow an irreversible
    /// call either (and so old records that name it still read).
    Model,
    /// Nobody answered in time.
    Timeout,
}

impl Resolver {
    pub const ALL: [Resolver; 5] = [
        Resolver::HumanTyped,
        Resolver::HumanKey,
        Resolver::Rule,
        Resolver::Model,
        Resolver::Timeout,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Resolver::HumanTyped => "human-typed",
            Resolver::HumanKey => "human-key",
            Resolver::Rule => "rule",
            Resolver::Model => "model",
            Resolver::Timeout => "timeout",
        }
    }
}

/// Whether a call of this reversibility needs the typed confirmation to be
/// allowed.
pub fn needs_typed_confirmation(tool: &str, subject: &str, cwd: &str) -> bool {
    reversibility(tool, subject, cwd) == Reversibility::R4
}

/// Whether `by` may end a call of reversibility `r` in `verdict`. Only a
/// typed confirmation may allow an irreversible call; everything may deny.
pub fn may_resolve(r: Reversibility, verdict: Verdict, by: Resolver) -> bool {
    !(verdict == Verdict::Allow && r == Reversibility::R4 && by != Resolver::HumanTyped)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_VERDICTS: [Verdict; 4] = [Verdict::Allow, Verdict::Deny, Verdict::Ask, Verdict::Pass];
    const ALL_TIERS: [Reversibility; 5] = [
        Reversibility::R0,
        Reversibility::R1,
        Reversibility::R2,
        Reversibility::R3,
        Reversibility::R4,
    ];

    #[test]
    fn only_a_typed_confirmation_allows_an_irreversible_call() {
        for by in Resolver::ALL {
            let ok = may_resolve(Reversibility::R4, Verdict::Allow, by);
            assert_eq!(ok, by == Resolver::HumanTyped, "{}", by.as_str());
        }
    }

    #[test]
    fn everything_may_deny_and_lesser_calls_are_not_held_back() {
        for by in Resolver::ALL {
            for verdict in ALL_VERDICTS {
                for r in ALL_TIERS {
                    let expect = !(verdict == Verdict::Allow && r == Reversibility::R4)
                        || by == Resolver::HumanTyped;
                    assert_eq!(may_resolve(r, verdict, by), expect);
                }
            }
        }
    }

    #[test]
    fn irreversible_calls_are_the_ones_that_need_it() {
        assert!(needs_typed_confirmation("Bash", "rm -rf build", "/w"));
        assert!(needs_typed_confirmation(
            "Bash",
            "git push --force origin x",
            "/w"
        ));
        assert!(!needs_typed_confirmation("Bash", "cargo test", "/w"));
        assert!(!needs_typed_confirmation("Read", "/w/a.rs", "/w"));
    }
}
