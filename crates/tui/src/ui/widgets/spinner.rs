//! Spinner frame sets, driven by an integer tick.

/// A family of spinner frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SpinnerSet {
    /// `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`
    #[default]
    Braille,
    /// `| / - \`
    Line,
    /// `⣾⣽⣻⢿⡿⣟⣯⣷`
    Dots,
}

const BRAILLE: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const LINE: [&str; 4] = ["|", "/", "-", "\\"];
const DOTS: [&str; 8] = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];

impl SpinnerSet {
    /// Every set, in a stable order.
    pub const ALL: [SpinnerSet; 3] = [SpinnerSet::Braille, SpinnerSet::Line, SpinnerSet::Dots];

    /// The frames of this set.
    pub fn frames(self) -> &'static [&'static str] {
        match self {
            SpinnerSet::Braille => &BRAILLE,
            SpinnerSet::Line => &LINE,
            SpinnerSet::Dots => &DOTS,
        }
    }

    /// The frame shown at `tick` when each frame lasts `ticks_per_frame`
    /// ticks (0 is treated as 1).
    pub fn frame(self, tick: u64, ticks_per_frame: u64) -> &'static str {
        let frames = self.frames();
        let len = u64::try_from(frames.len()).unwrap_or(1).max(1);
        let index = (tick / ticks_per_frame.max(1)) % len;
        usize::try_from(index)
            .ok()
            .and_then(|i| frames.get(i))
            .copied()
            .unwrap_or("")
    }

    /// The set called `name` (case-insensitive, surrounding space ignored).
    pub fn parse(name: &str) -> Option<SpinnerSet> {
        let name = name.trim();
        SpinnerSet::ALL
            .into_iter()
            .find(|set| set.as_str().eq_ignore_ascii_case(name))
    }

    /// The name used in configuration.
    pub fn as_str(self) -> &'static str {
        match self {
            SpinnerSet::Braille => "braille",
            SpinnerSet::Line => "line",
            SpinnerSet::Dots => "dots",
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{buffer::Buffer, layout::Rect, style::Color, widgets::Widget};

    use super::*;

    #[test]
    fn frames_cycle_at_the_requested_pace() {
        assert_eq!(SpinnerSet::Braille.frames().concat(), "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏");
        assert_eq!(SpinnerSet::Dots.frames().concat(), "⣾⣽⣻⢿⡿⣟⣯⣷");
        assert_eq!(SpinnerSet::Line.frames(), ["|", "/", "-", "\\"]);
        assert_eq!(SpinnerSet::Line.frame(0, 2), "|");
        assert_eq!(SpinnerSet::Line.frame(3, 2), "/");
        assert_eq!(SpinnerSet::Line.frame(8, 2), "|");
        assert_eq!(SpinnerSet::Braille.frame(11, 0), "⠙");
        assert_eq!(SpinnerSet::Dots.frame(u64::MAX, 1), "⣷");
    }

    #[test]
    fn names_round_trip() {
        for set in SpinnerSet::ALL {
            assert_eq!(SpinnerSet::parse(set.as_str()), Some(set));
        }
        assert_eq!(SpinnerSet::parse(" DOTS "), Some(SpinnerSet::Dots));
        assert_eq!(SpinnerSet::parse("nope"), None);
        assert_eq!(SpinnerSet::default(), SpinnerSet::Braille);
    }

    #[test]
    fn frames_render_as_single_cells() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        ratatui::text::Span::styled(SpinnerSet::Dots.frame(2, 1), Color::Green)
            .render(buf.area, &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "⣻");
        assert_eq!(buf[(0, 0)].fg, Color::Green);
    }

    #[test]
    fn tick_fuzz_never_panics() {
        for set in SpinnerSet::ALL {
            for tick in (0..=40).chain([u64::MAX - 1, u64::MAX]) {
                for pace in [0, 1, 3, u64::MAX] {
                    assert!(!set.frame(tick, pace).is_empty());
                }
            }
        }
    }
}
