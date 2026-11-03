//! Bounded deferral: how long an item may wait before it must be put in
//! front of the user, and what lets it through earlier.

use crate::{Millis, Priority};

/// P1 is shown at once, within this.
pub const P1_BOUND_MS: Millis = 15_000;
/// P2 waits for a breakpoint, never longer than this.
pub const P2_BOUND_MS: Millis = 2 * 60_000;
/// P3 is batched at breakpoints, never longer than this.
pub const P3_BOUND_MS: Millis = 15 * 60_000;
/// In focus mode P3 waits until focus ends, never longer than this.
pub const P3_FOCUS_BOUND_MS: Millis = 45 * 60_000;
/// Quiet this long counts as a breakpoint.
pub const IDLE_BREAKPOINT_MS: Millis = 10_000;

/// A moment where interrupting costs little, as far as swarmux can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Breakpoint {
    PromptSent,
    DiffReviewed,
    FocusClosed,
    TileSwitched,
    Idle,
}

/// The longest an item of `priority` may be held back.
pub fn bound(priority: Priority, focus: bool) -> Millis {
    match (priority, focus) {
        (Priority::P1, _) => P1_BOUND_MS,
        (Priority::P2, _) => P2_BOUND_MS,
        (Priority::P3, false) => P3_BOUND_MS,
        (Priority::P3, true) => P3_FOCUS_BOUND_MS,
    }
}

/// When an item that started needing the user at `since` must be shown.
pub fn deliver_by(since: Millis, priority: Priority, focus: bool) -> Millis {
    since.saturating_add(bound(priority, focus))
}

/// Whether a breakpoint lets items of `priority` through early. In focus
/// mode P3 waits for focus to end, not for a breakpoint.
pub fn releases(priority: Priority, _breakpoint: Breakpoint, focus: bool) -> bool {
    match priority {
        Priority::P1 | Priority::P2 => true,
        Priority::P3 => !focus,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_grow_with_patience() {
        assert_eq!(bound(Priority::P1, true), 15_000);
        assert_eq!(bound(Priority::P2, false), 120_000);
        assert_eq!(bound(Priority::P3, false), 900_000);
        assert_eq!(bound(Priority::P3, true), 2_700_000);
        assert_eq!(deliver_by(1_000, Priority::P2, true), 121_000);
    }

    #[test]
    fn breakpoints_release_p2_and_p3_outside_focus_only() {
        let bp = Breakpoint::TileSwitched;
        assert!(releases(Priority::P2, bp, true));
        assert!(releases(Priority::P3, bp, false));
        assert!(!releases(Priority::P3, bp, true));
    }
}
