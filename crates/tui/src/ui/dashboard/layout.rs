//! How much of the dashboard fits: panels collapse as the window shrinks.

/// Layouts from the full dashboard down to a "make it bigger" note.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// Below 80×24.
    TooSmall,
    /// Reviewer, lead and guard as one-liners, subagents as a list, log only.
    Minimal,
    /// Reviewer as a one-liner, subagents as a list.
    Compact,
    /// Everything but the who-sees-what panel.
    NoWho,
    /// Everything (150×45 and up).
    Full,
}

impl Tier {
    /// The tier for a terminal `w` columns by `h` rows.
    pub fn for_size(w: u16, h: u16) -> Tier {
        match (w, h) {
            (w, h) if w < 80 || h < 24 => Tier::TooSmall,
            (150.., 45..) => Tier::Full,
            (120.., 38..) => Tier::NoWho,
            (100.., 30..) => Tier::Compact,
            _ => Tier::Minimal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_by_size() {
        assert_eq!(Tier::for_size(160, 48), Tier::Full);
        assert_eq!(Tier::for_size(150, 45), Tier::Full);
        assert_eq!(Tier::for_size(149, 48), Tier::NoWho);
        assert_eq!(Tier::for_size(200, 40), Tier::NoWho);
        assert_eq!(Tier::for_size(100, 30), Tier::Compact);
        assert_eq!(Tier::for_size(250, 32), Tier::Compact);
        assert_eq!(Tier::for_size(80, 24), Tier::Minimal);
        assert_eq!(Tier::for_size(99, 60), Tier::Minimal);
        assert_eq!(Tier::for_size(79, 60), Tier::TooSmall);
        assert_eq!(Tier::for_size(200, 23), Tier::TooSmall);
    }
}
