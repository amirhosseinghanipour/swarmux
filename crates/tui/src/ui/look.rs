//! The look: theme, icons, gaps, layouts and frame rate, resolved once from
//! the settings and changed at runtime with `t` and `L`.

use std::time::Duration;

use swarmux_application::UiSettings;

use super::grid::layout::{self, LayoutSpec};
use super::widgets::SpinnerSet;
use crate::theme::{ColorDepth, Theme};

/// Frame rate in low-power mode.
pub const LOW_POWER_FPS: u16 = 4;

/// Which glyphs panel titles use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IconSet {
    /// Plain ASCII marks that every font has.
    #[default]
    Ascii,
    /// Geometric Unicode marks that ordinary monospace fonts have.
    Unicode,
    /// Nerd Font glyphs.
    Nerd,
}

/// Something with an icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Lead,
    Reviewer,
    Guard,
    Subagents,
    Back,
    Timeline,
    Who,
    Log,
    Steps,
    Usage,
    Burn,
    Tokens,
    Context,
    Agents,
    Todos,
    Tools,
    Files,
    Git,
    Ci,
    System,
    Wall,
    Calendar,
    Env,
    Mix,
    Theme,
    /// Mission control: the attention inbox and the change log.
    Inbox,
    Changes,
    /// What an inbox item is about.
    Approval,
    Question,
    Failed,
    Concerns,
    Result,
    Stalled,
    /// A session's state on the wall.
    StateRun,
    StateWait,
    StateDone,
    StateErr,
    StateIdle,
    /// Where a window's time has got to on a gauge.
    Pace,
    /// Focus: a tool call, a checkpoint, a human event, "you were here".
    Call,
    Checkpoint,
    Human,
    Here,
}

impl IconSet {
    pub fn parse(s: &str) -> Option<IconSet> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ascii" => Some(IconSet::Ascii),
            "unicode" => Some(IconSet::Unicode),
            "nerd" => Some(IconSet::Nerd),
            _ => None,
        }
    }

    pub fn get(self, icon: Icon) -> &'static str {
        use Icon::*;
        match self {
            IconSet::Ascii => match icon {
                Lead => ">",
                Reviewer => "%",
                Guard => "#",
                Subagents => "&",
                Back => "<",
                Timeline => "~",
                Who => "=",
                Log => "$",
                Steps => "-",
                Usage => "%",
                Burn => "^",
                Tokens => "~",
                Context => "@",
                Agents => "+",
                Todos => "*",
                Tools => "!",
                Files => "/",
                Git => "+",
                Ci => "?",
                System => "#",
                Wall => "#",
                Calendar => ":",
                Env => "=",
                Mix => "o",
                Theme => "*",
                Inbox => "!",
                Changes => "~",
                Approval => "!",
                Question => "?",
                Failed => "x",
                Concerns => "~",
                Result => "+",
                Stalled => "z",
                StateRun => "*",
                StateWait => "?",
                StateDone => "+",
                StateErr => "x",
                StateIdle => "-",
                Pace => "|",
                Call => ">",
                Checkpoint => "o",
                Human => "^",
                Here => ">",
            },
            IconSet::Unicode => match icon {
                Lead => "▸",
                Reviewer => "◉",
                Guard => "◆",
                Subagents => "❖",
                Back => "◂",
                Timeline => "≈",
                Who => "≡",
                Log => "›",
                Steps => "·",
                Usage => "◔",
                Burn => "▲",
                Tokens => "⇄",
                Context => "◍",
                Agents => "┬",
                Todos => "✓",
                Tools => "✦",
                Files => "▤",
                Git => "⎇",
                Ci => "✔",
                System => "▦",
                Wall => "▦",
                Calendar => "▥",
                Env => "≡",
                Mix => "◔",
                Theme => "◐",
                Inbox => "◈",
                Changes => "≋",
                Approval => "◆",
                Question => "?",
                Failed => "✗",
                Concerns => "△",
                Result => "✓",
                Stalled => "◌",
                StateRun => "●",
                StateWait => "◐",
                StateDone => "✓",
                StateErr => "✗",
                StateIdle => "○",
                Pace => "┃",
                Call => "▸",
                Checkpoint => "◇",
                Human => "▲",
                Here => "▶",
            },
            IconSet::Nerd => match icon {
                Lead => "\u{f06a9}",      // md-robot
                Reviewer => "\u{f06e}",   // fa-eye
                Guard => "\u{f0565}",     // md-shield_check
                Subagents => "\u{f0849}", // md-account_group
                Back => "\u{f112}",       // fa-reply
                Timeline => "\u{f017}",   // fa-clock
                Who => "\u{f0c0}",        // fa-users
                Log => "\u{f120}",        // fa-terminal
                Steps => "\u{f03a}",      // fa-list
                Usage => "\u{f029a}",     // md-gauge
                Burn => "\u{f0238}",      // md-fire
                Tokens => "\u{f04e1}",    // md-swap_horizontal
                Context => "\u{f09d1}",   // md-brain
                Agents => "\u{f0645}",    // md-file_tree
                Todos => "\u{f046}",      // fa-check_square
                Tools => "\u{f0ad}",      // fa-wrench
                Files => "\u{f0f6}",      // fa-file_text
                Git => "\u{e725}",        // dev-git_branch
                Ci => "\u{f407}",         // oct-git_pull_request
                System => "\u{f2db}",     // fa-microchip
                Wall => "\u{f0570}",      // md-view_grid
                Calendar => "\u{f073}",   // fa-calendar
                Env => "\u{f013}",        // fa-cog
                Mix => "\u{f0200}",       // md-chart_donut
                Theme => "\u{f03d8}",     // md-palette
                Inbox => "\u{f01c}",      // fa-inbox
                Changes => "\u{f1da}",    // fa-history
                Approval => "\u{f132}",   // fa-shield
                Question => "\u{f128}",   // fa-question
                Failed => "\u{f00d}",     // fa-times
                Concerns => "\u{f071}",   // fa-warning
                Result => "\u{f00c}",     // fa-check
                Stalled => "\u{f254}",    // fa-hourglass
                StateRun => "\u{f04b}",   // fa-play
                StateWait => "\u{f254}",  // fa-hourglass
                StateDone => "\u{f00c}",  // fa-check
                StateErr => "\u{f00d}",   // fa-times
                StateIdle => "\u{f10c}",  // fa-circle_o
                Pace => "┃",
                Call => "\u{f054}",       // fa-chevron_right
                Checkpoint => "\u{f10c}", // fa-circle_o
                Human => "\u{f007}",      // fa-user
                Here => "\u{f0da}",       // fa-caret_right
            },
        }
    }

    /// Spinners match the glyph set: braille with Nerd Fonts, `[|]` otherwise.
    pub fn spinner(self) -> SpinnerSet {
        match self {
            IconSet::Ascii => SpinnerSet::Line,
            IconSet::Unicode | IconSet::Nerd => SpinnerSet::Braille,
        }
    }
}

/// Everything about how swarmux looks, resolved from [`UiSettings`].
#[derive(Clone, Debug)]
pub struct Look {
    pub theme: Theme,
    pub depth: ColorDepth,
    pub icons: IconSet,
    pub gaps: u16,
    fps: u16,
    pub low_power: bool,
    /// Presets first, then the user's.
    pub layouts: Vec<LayoutSpec>,
    /// Index into `layouts`.
    pub layout: usize,
    /// The splash is up until this frame (`None`: dismissed or off).
    pub splash_until: Option<u64>,
}

impl Default for Look {
    fn default() -> Look {
        Look {
            theme: Theme::default(),
            depth: ColorDepth::TrueColor,
            icons: IconSet::Ascii,
            gaps: 1,
            fps: 10,
            low_power: false,
            layouts: layout::presets(),
            layout: 0,
            splash_until: None,
        }
    }
}

impl Look {
    /// The look the settings ask for, and what in them was not understood.
    pub fn from_settings(ui: &UiSettings, depth: ColorDepth) -> (Look, Vec<String>) {
        let mut problems = Vec::new();
        let theme = Theme::named(&ui.theme).unwrap_or_else(|| {
            problems.push(format!("unknown theme \"{}\", using reference", ui.theme));
            Theme::default()
        });
        let icons = IconSet::parse(&ui.icons).unwrap_or_default();
        let mut layouts = layout::presets();
        for user in &ui.layouts {
            match layout::from_settings(user) {
                Ok(spec) => {
                    layouts.retain(|l| l.name != spec.name);
                    layouts.push(spec);
                }
                Err(e) => problems.push(format!("layout \"{}\": {e}", user.name)),
            }
        }
        let layout = layouts
            .iter()
            .position(|l| l.name == ui.layout.trim())
            .unwrap_or_else(|| {
                problems.push(format!("unknown layout \"{}\", using command", ui.layout));
                0
            });
        let fps = ui.fps.clamp(1, 30);
        let look = Look {
            theme,
            depth,
            icons,
            gaps: ui.gaps.min(4),
            fps,
            low_power: ui.low_power,
            layouts,
            layout,
            splash_until: None,
        };
        (look, problems)
    }

    /// Frames per second actually drawn.
    pub fn fps(&self) -> u16 {
        if self.low_power {
            LOW_POWER_FPS
        } else {
            self.fps
        }
    }

    /// Time between frames.
    pub fn frame_time(&self) -> Duration {
        Duration::from_millis(1000 / u64::from(self.fps().max(1)))
    }

    /// Frames in `ms` milliseconds (at least one).
    pub fn frames(&self, ms: u64) -> u64 {
        (ms * u64::from(self.fps()) / 1000).max(1)
    }

    /// Whether tweens and pulses run.
    pub fn animated(&self) -> bool {
        !self.low_power
    }

    pub fn layout_spec(&self) -> &LayoutSpec {
        self.layouts
            .get(self.layout)
            .or_else(|| self.layouts.first())
            .unwrap_or_else(|| layout::fallback())
    }

    pub fn next_theme(&mut self) -> &Theme {
        self.theme = self.theme.next();
        &self.theme
    }

    pub fn next_layout(&mut self) -> &LayoutSpec {
        self.layout = (self.layout + 1) % self.layouts.len().max(1);
        self.layout_spec()
    }

    pub fn spinner(&self) -> SpinnerSet {
        self.icons.spinner()
    }

    /// Whether the splash shows at `frame`.
    pub fn splash_at(&self, frame: u64) -> bool {
        self.splash_until.is_some_and(|end| frame < end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_application::{LayoutSettings, RowSettings};

    #[test]
    fn defaults_are_the_reference_command_look() {
        let (look, problems) = Look::from_settings(&UiSettings::default(), ColorDepth::TrueColor);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(look.theme.name, "reference");
        assert_eq!(look.layout_spec().name, "command");
        assert_eq!((look.icons, look.gaps, look.fps()), (IconSet::Ascii, 1, 10));
        assert_eq!(look.frame_time(), Duration::from_millis(100));
    }

    #[test]
    fn low_power_drops_to_four_fps_and_stops_animation() {
        let ui = UiSettings {
            low_power: true,
            fps: 30,
            ..UiSettings::default()
        };
        let (look, _) = Look::from_settings(&ui, ColorDepth::TrueColor);
        assert_eq!(look.fps(), 4);
        assert!(!look.animated());
        assert_eq!(look.frames(1500), 6);
    }

    #[test]
    fn unknown_names_fall_back_and_are_reported() {
        let ui = UiSettings {
            theme: "solarized".into(),
            layout: "mine".into(),
            ..UiSettings::default()
        };
        let (look, problems) = Look::from_settings(&ui, ColorDepth::TrueColor);
        assert_eq!(look.theme.name, "reference");
        assert_eq!(look.layout_spec().name, "command");
        assert_eq!(problems.len(), 2, "{problems:?}");
    }

    #[test]
    fn user_layouts_join_the_cycle() {
        let ui = UiSettings {
            layout: "focus".into(),
            layouts: vec![LayoutSettings {
                name: "focus".into(),
                rows: vec![RowSettings {
                    panels: vec!["lead:2".into(), "log".into()],
                    height: None,
                    weight: None,
                }],
            }],
            ..UiSettings::default()
        };
        let (mut look, problems) = Look::from_settings(&ui, ColorDepth::TrueColor);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(look.layout_spec().name, "focus");
        assert_eq!(look.next_layout().name, "command", "wraps to the first");
        let names: Vec<_> = look.layouts.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["command", "btop", "minimal", "focus"]);
    }

    #[test]
    fn icon_sets() {
        assert_eq!(IconSet::parse("NERD"), Some(IconSet::Nerd));
        assert_eq!(IconSet::parse("Unicode"), Some(IconSet::Unicode));
        assert_eq!(IconSet::Ascii.get(Icon::Guard), "#");
        assert!(IconSet::Ascii.get(Icon::Lead).is_ascii());
        assert!(!IconSet::Nerd.get(Icon::Lead).is_ascii());
        assert_eq!(IconSet::Nerd.spinner(), SpinnerSet::Braille);
    }
}
