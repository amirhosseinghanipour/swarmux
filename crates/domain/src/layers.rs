//! The layers configuration comes from, and how a key's values from
//! several layers become one.
//!
//! Pure vocabulary: the tiers in order of precedence, the merge kinds, what
//! a key may do (grant, restrict) and which tiers may set it. Reading files
//! and merging values is the infrastructure's job.

use std::fmt;

/// Where a value came from, lowest precedence first (the managed tier's
/// locks are above all of them).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    /// Compiled in: the defaults and the integrity invariants.
    Core,
    /// The organisation's file, for keys it did not lock: a default the
    /// user may change.
    Org,
    /// The user's `config.toml`.
    User,
    /// A workspace's own file, under the config folder.
    Workspace,
    /// The repository's `.swarmux/*.toml`.
    Project,
    /// An installed practice pack (placeholder: no pack format yet).
    Pack,
    /// The mission contract.
    Mission,
    /// Command-line flags and `SWARMUX_<SECTION>__<KEY>` variables.
    Run,
    /// The organisation's file, for the keys it locked: wins over all.
    Managed,
}

impl Tier {
    pub const ALL: [Tier; 9] = [
        Tier::Core,
        Tier::Org,
        Tier::User,
        Tier::Workspace,
        Tier::Project,
        Tier::Pack,
        Tier::Mission,
        Tier::Run,
        Tier::Managed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Core => "core",
            Tier::Org => "org",
            Tier::User => "user",
            Tier::Workspace => "workspace",
            Tier::Project => "project",
            Tier::Pack => "pack",
            Tier::Mission => "mission",
            Tier::Run => "run",
            Tier::Managed => "managed",
        }
    }

    pub fn parse(s: &str) -> Option<Tier> {
        Tier::ALL.into_iter().find(|t| t.as_str() == s)
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How the values of one key from different tiers become one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeKind {
    /// The nearest (highest) tier wins, unless the managed tier locked it.
    Scalar,
    /// The union of every tier's list, in tier order, without repeats.
    List,
    /// The strictest value of any tier wins.
    Lock,
    /// Taken whole from the highest tier that sets it.
    Allowlist,
}

impl MergeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MergeKind::Scalar => "scalar",
            MergeKind::List => "list",
            MergeKind::Lock => "lock",
            MergeKind::Allowlist => "allowlist",
        }
    }
}

/// What a key does to what swarmux allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Neither: a look-and-feel or tuning value.
    Neutral,
    /// Allows more (allow rules, commands, endpoints, notification
    /// targets): a repository's value counts only with the owner's trust.
    Grants,
    /// Allows less (protected paths, denies, stricter limits): counts
    /// from every tier at once.
    Restricts,
}

impl Effect {
    pub fn as_str(self) -> &'static str {
        match self {
            Effect::Neutral => "neutral",
            Effect::Grants => "grants",
            Effect::Restricts => "restricts",
        }
    }
}

/// A set of tiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TierSet(u16);

impl TierSet {
    pub const NONE: TierSet = TierSet(0);

    pub const fn of(tiers: &[Tier]) -> TierSet {
        let mut bits = 0u16;
        let mut i = 0;
        while i < tiers.len() {
            bits |= 1 << tiers[i] as u16;
            i += 1;
        }
        TierSet(bits)
    }

    pub fn contains(self, tier: Tier) -> bool {
        self.0 & (1 << tier as u16) != 0
    }

    pub fn with(self, tier: Tier) -> TierSet {
        TierSet(self.0 | (1 << tier as u16))
    }

    pub fn without(self, tier: Tier) -> TierSet {
        TierSet(self.0 & !(1 << tier as u16))
    }

    /// The tiers in the set, lowest first.
    pub fn list(self) -> Vec<Tier> {
        Tier::ALL
            .into_iter()
            .filter(|t| self.contains(*t))
            .collect()
    }
}

/// Where a decision's rule came from: for the ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub tier: Tier,
    /// The file (or `built-in`, `always-allow`) it was read from.
    pub source: String,
}

impl Origin {
    pub fn core() -> Origin {
        Origin {
            tier: Tier::Core,
            source: "built-in".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_are_ordered_lowest_first_and_named() {
        assert!(Tier::Core < Tier::User && Tier::User < Tier::Project && Tier::Project < Tier::Run);
        assert!(Tier::Run < Tier::Managed);
        for t in Tier::ALL {
            assert_eq!(Tier::parse(t.as_str()), Some(t));
        }
    }

    #[test]
    fn a_tier_set_holds_tiers() {
        let set = TierSet::of(&[Tier::User, Tier::Run]);
        assert!(set.contains(Tier::User) && set.contains(Tier::Run));
        assert!(!set.contains(Tier::Project));
        assert_eq!(
            set.with(Tier::Project).list(),
            [Tier::User, Tier::Project, Tier::Run]
        );
        assert!(!set.without(Tier::User).contains(Tier::User));
    }
}
