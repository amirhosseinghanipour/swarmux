//! The Claude Code versions swarmux's guarantees were checked against.
//!
//! The honesty rule applies here: a number in this file is either something
//! a canary or the real `claude` was seen doing (the tested range), or a
//! floor taken from Claude Code's changelog, and it is marked which.

use swarmux_domain::runtime::{Version, VersionRange};

/// The oldest Claude Code with `onFailure: "block"` for command hooks, from
/// its changelog (2.1.295, 2026-10-08). Without it a hook that fails (exit
/// 1, a timeout, a crash) lets the call run, so the Guard cannot fail closed.
/// *Docs-only*: not run on a version below the tested range.
pub const FLOOR_ON_FAILURE: Version = Version::new(2, 1, 295);

/// On Windows a PreToolUse hook that set no shell failed open when Git for
/// Windows was not installed until 2.1.299 (changelog, 2026-10-14).
/// *Docs-only.*
pub const FLOOR_WINDOWS: Version = Version::new(2, 1, 299);

/// The oldest and the newest version the canaries and the launch profiles
/// were run against, on the machine that wrote this. *Observed*: 2.1.323 is
/// the version `claude --version` printed during the spike for X3 (see
/// `guide/claude-code.md`); nothing older or newer was run.
pub const TESTED_MIN: Version = Version::new(2, 1, 323);
pub const TESTED_MAX: Version = Version::new(2, 1, 323);

/// The floor for the platform swarmux runs on.
pub const fn floor() -> Version {
    if cfg!(windows) {
        FLOOR_WINDOWS
    } else {
        FLOOR_ON_FAILURE
    }
}

/// The range used to judge a reported version.
pub const fn range() -> VersionRange {
    VersionRange {
        floor: floor(),
        tested_min: TESTED_MIN,
        tested_max: TESTED_MAX,
    }
}

/// Whether `version` knows `onFailure` for command hooks (2.1.295 or newer).
pub fn knows_on_failure(version: Version) -> bool {
    version >= FLOOR_ON_FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_domain::runtime::VersionStanding;

    #[test]
    fn the_range_is_ordered_and_the_floor_is_under_it() {
        assert!(floor() <= TESTED_MIN);
        assert!(TESTED_MIN <= TESTED_MAX);
        assert!(FLOOR_ON_FAILURE <= FLOOR_WINDOWS);
    }

    #[test]
    fn standing_follows_the_declared_range() {
        let at = |v| range().standing(Some(v));
        assert_eq!(at(Version::new(2, 1, 294)), VersionStanding::BelowFloor);
        assert_eq!(at(TESTED_MIN), VersionStanding::Tested);
        assert_eq!(at(TESTED_MAX), VersionStanding::Tested);
        assert_eq!(at(Version::new(2, 1, 400)), VersionStanding::AboveTested);
        assert_eq!(range().standing(None), VersionStanding::Unknown);
    }

    #[test]
    fn on_failure_starts_at_2_1_295() {
        assert!(!knows_on_failure(Version::new(2, 1, 294)));
        assert!(knows_on_failure(Version::new(2, 1, 295)));
        assert!(knows_on_failure(Version::new(2, 2, 0)));
    }
}
