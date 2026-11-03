//! Colour themes: every colour the interface uses comes from a [`Theme`]
//! role, never from a literal. Themes are plain data; `t` cycles them.

use ratatui::{buffer::Buffer, style::Color};
use swarmux_domain::Family;

/// Semantic colour roles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    /// The name used in `config.toml` and the toast.
    pub name: &'static str,
    /// The screen background.
    pub bg: Color,
    /// Inside panels (often the same as `bg`).
    pub panel_bg: Color,
    /// Panel borders.
    pub border: Color,
    /// The border of the focused panel.
    pub border_focus: Color,
    /// Bold headings such as `SWARMUX`.
    pub title: Color,
    /// Body text.
    pub text: Color,
    /// Secondary text: hints, counters, paths.
    pub muted: Color,
    /// Labels in front of values.
    pub label: Color,
    /// Placeholders and empty tracks.
    pub dim: Color,
    /// The lead (Opus and unknown models).
    pub lead: Color,
    /// Builders (Sonnet).
    pub builder: Color,
    /// Haiku.
    pub haiku: Color,
    /// The Guard / JEV panel.
    pub guard: Color,
    /// The reviewer when its model is unknown.
    pub reviewer: Color,
    pub ok: Color,
    pub warn: Color,
    pub error: Color,
    /// Text on an accent background (chips, the active tab).
    pub on_accent: Color,
    /// Gauges and bars: low → mid → high.
    pub gauge: [Color; 3],
    /// Charts, the logo and heatmaps: cool → warm.
    pub chart: [Color; 3],
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// The default look: OLED black, orange lead, blue Haiku, green guard.
pub const REFERENCE: Theme = Theme {
    name: "reference",
    bg: Color::Rgb(0, 0, 0),
    panel_bg: Color::Rgb(0, 0, 0),
    border: Color::Rgb(120, 120, 140),
    border_focus: Color::Rgb(232, 140, 70),
    title: Color::Rgb(240, 238, 245),
    text: Color::Rgb(214, 211, 224),
    muted: Color::Rgb(120, 120, 140),
    label: Color::Rgb(150, 140, 180),
    dim: Color::Rgb(75, 72, 90),
    lead: Color::Rgb(232, 140, 70),
    builder: Color::Rgb(232, 140, 70),
    haiku: Color::Rgb(110, 160, 235),
    guard: Color::Rgb(110, 200, 130),
    reviewer: Color::Rgb(150, 140, 180),
    ok: Color::Rgb(110, 200, 130),
    warn: Color::Rgb(232, 140, 70),
    error: Color::Rgb(230, 90, 90),
    on_accent: Color::Rgb(0, 0, 0),
    gauge: [
        Color::Rgb(110, 200, 130),
        Color::Rgb(232, 140, 70),
        Color::Rgb(230, 90, 90),
    ],
    chart: [
        Color::Rgb(110, 160, 235),
        Color::Rgb(150, 140, 180),
        Color::Rgb(232, 140, 70),
    ],
};

/// Catppuccin Mocha.
pub const CATPPUCCIN_MOCHA: Theme = Theme {
    name: "catppuccin-mocha",
    bg: rgb(0x11111b),           // crust
    panel_bg: rgb(0x1e1e2e),     // base
    border: rgb(0x585b70),       // surface2
    border_focus: rgb(0xcba6f7), // mauve
    title: rgb(0xcdd6f4),        // text
    text: rgb(0xcdd6f4),
    muted: rgb(0x7f849c),    // overlay1
    label: rgb(0xa6adc8),    // subtext0
    dim: rgb(0x45475a),      // surface1
    lead: rgb(0xfab387),     // peach
    builder: rgb(0xf9e2af),  // yellow
    haiku: rgb(0x89b4fa),    // blue
    guard: rgb(0xa6e3a1),    // green
    reviewer: rgb(0xcba6f7), // mauve
    ok: rgb(0xa6e3a1),
    warn: rgb(0xf9e2af),
    error: rgb(0xf38ba8), // red
    on_accent: rgb(0x11111b),
    gauge: [rgb(0xa6e3a1), rgb(0xf9e2af), rgb(0xf38ba8)],
    chart: [rgb(0x89b4fa), rgb(0xcba6f7), rgb(0xf5c2e7)],
};

/// Tokyo Night (night).
pub const TOKYO_NIGHT: Theme = Theme {
    name: "tokyo-night",
    bg: rgb(0x16161e),           // bg_dark
    panel_bg: rgb(0x1a1b26),     // bg
    border: rgb(0x414868),       // terminal_black
    border_focus: rgb(0x7aa2f7), // blue
    title: rgb(0xc0caf5),        // fg
    text: rgb(0xc0caf5),
    muted: rgb(0x565f89),    // comment
    label: rgb(0xa9b1d6),    // fg_dark
    dim: rgb(0x292e42),      // bg_highlight
    lead: rgb(0xff9e64),     // orange
    builder: rgb(0xe0af68),  // yellow
    haiku: rgb(0x7aa2f7),    // blue
    guard: rgb(0x9ece6a),    // green
    reviewer: rgb(0xbb9af7), // magenta
    ok: rgb(0x9ece6a),
    warn: rgb(0xe0af68),
    error: rgb(0xf7768e), // red
    on_accent: rgb(0x16161e),
    gauge: [rgb(0x9ece6a), rgb(0xe0af68), rgb(0xf7768e)],
    chart: [rgb(0x7dcfff), rgb(0x7aa2f7), rgb(0xbb9af7)],
};

/// Gruvbox dark (hard contrast background).
pub const GRUVBOX_DARK: Theme = Theme {
    name: "gruvbox-dark",
    bg: rgb(0x1d2021),           // bg0_h
    panel_bg: rgb(0x282828),     // bg0
    border: rgb(0x665c54),       // bg3
    border_focus: rgb(0xfabd2f), // yellow
    title: rgb(0xfbf1c7),        // fg0
    text: rgb(0xebdbb2),         // fg
    muted: rgb(0x928374),        // gray
    label: rgb(0xa89984),        // fg4
    dim: rgb(0x504945),          // bg2
    lead: rgb(0xfe8019),         // orange
    builder: rgb(0xfabd2f),      // yellow
    haiku: rgb(0x83a598),        // blue
    guard: rgb(0xb8bb26),        // green
    reviewer: rgb(0xd3869b),     // purple
    ok: rgb(0xb8bb26),
    warn: rgb(0xfabd2f),
    error: rgb(0xfb4934), // red
    on_accent: rgb(0x1d2021),
    gauge: [rgb(0xb8bb26), rgb(0xfabd2f), rgb(0xfb4934)],
    chart: [rgb(0x83a598), rgb(0x8ec07c), rgb(0xfe8019)],
};

/// Rosé Pine (main).
pub const ROSE_PINE: Theme = Theme {
    name: "rose-pine",
    bg: rgb(0x191724),           // base
    panel_bg: rgb(0x1f1d2e),     // surface
    border: rgb(0x524f67),       // highlight high
    border_focus: rgb(0xebbcba), // rose
    title: rgb(0xe0def4),        // text
    text: rgb(0xe0def4),
    muted: rgb(0x6e6a86),    // muted
    label: rgb(0x908caa),    // subtle
    dim: rgb(0x403d52),      // highlight med
    lead: rgb(0xebbcba),     // rose
    builder: rgb(0xf6c177),  // gold
    haiku: rgb(0x9ccfd8),    // foam
    guard: rgb(0x31748f),    // pine
    reviewer: rgb(0xc4a7e7), // iris
    ok: rgb(0x9ccfd8),
    warn: rgb(0xf6c177),
    error: rgb(0xeb6f92), // love
    on_accent: rgb(0x191724),
    gauge: [rgb(0x9ccfd8), rgb(0xf6c177), rgb(0xeb6f92)],
    chart: [rgb(0x31748f), rgb(0xc4a7e7), rgb(0xebbcba)],
};

/// Nord.
pub const NORD: Theme = Theme {
    name: "nord",
    bg: rgb(0x2e3440), // nord0
    panel_bg: rgb(0x2e3440),
    border: rgb(0x4c566a),       // nord3
    border_focus: rgb(0x88c0d0), // nord8
    title: rgb(0xeceff4),        // nord6
    text: rgb(0xd8dee9),         // nord4
    muted: rgb(0x7b88a1),        // nord3 bright (comments)
    label: rgb(0x81a1c1),        // nord9
    dim: rgb(0x434c5e),          // nord2
    lead: rgb(0xd08770),         // nord12
    builder: rgb(0xebcb8b),      // nord13
    haiku: rgb(0x88c0d0),        // nord8
    guard: rgb(0xa3be8c),        // nord14
    reviewer: rgb(0xb48ead),     // nord15
    ok: rgb(0xa3be8c),
    warn: rgb(0xebcb8b),
    error: rgb(0xbf616a), // nord11
    on_accent: rgb(0x2e3440),
    gauge: [rgb(0xa3be8c), rgb(0xebcb8b), rgb(0xbf616a)],
    chart: [rgb(0x5e81ac), rgb(0x88c0d0), rgb(0x8fbcbb)],
};

/// Dracula.
pub const DRACULA: Theme = Theme {
    name: "dracula",
    bg: rgb(0x21222c),
    panel_bg: rgb(0x282a36),     // background
    border: rgb(0x44475a),       // current line
    border_focus: rgb(0xbd93f9), // purple
    title: rgb(0xf8f8f2),        // foreground
    text: rgb(0xf8f8f2),
    muted: rgb(0x6272a4), // comment
    label: rgb(0xbd93f9), // purple
    dim: rgb(0x44475a),
    lead: rgb(0xffb86c),     // orange
    builder: rgb(0xf1fa8c),  // yellow
    haiku: rgb(0x8be9fd),    // cyan
    guard: rgb(0x50fa7b),    // green
    reviewer: rgb(0xff79c6), // pink
    ok: rgb(0x50fa7b),
    warn: rgb(0xf1fa8c),
    error: rgb(0xff5555), // red
    on_accent: rgb(0x282a36),
    gauge: [rgb(0x50fa7b), rgb(0xf1fa8c), rgb(0xff5555)],
    chart: [rgb(0x8be9fd), rgb(0xbd93f9), rgb(0xff79c6)],
};

/// Every theme, in the order `t` cycles them.
pub const THEMES: [Theme; 7] = [
    REFERENCE,
    CATPPUCCIN_MOCHA,
    TOKYO_NIGHT,
    GRUVBOX_DARK,
    ROSE_PINE,
    NORD,
    DRACULA,
];

impl Default for Theme {
    fn default() -> Theme {
        REFERENCE
    }
}

impl Theme {
    /// The theme called `name` (case and `_`/space vs `-` don't matter).
    pub fn named(name: &str) -> Option<Theme> {
        let want: String = name
            .trim()
            .to_ascii_lowercase()
            .chars()
            .map(|c| if c == '_' || c == ' ' { '-' } else { c })
            .collect();
        THEMES.into_iter().find(|t| t.name == want)
    }

    /// Index of this theme in [`THEMES`].
    pub fn index(&self) -> usize {
        THEMES.iter().position(|t| t.name == self.name).unwrap_or(0)
    }

    /// The theme after this one.
    pub fn next(&self) -> Theme {
        THEMES[(self.index() + 1) % THEMES.len()]
    }

    /// The accent for a model: lead for Opus and unknown families, builder
    /// for Sonnet, haiku for Haiku, muted when there is no model.
    pub fn model(&self, model: Option<&str>) -> Color {
        match model.map(Family::of) {
            Some(Family::Haiku) => self.haiku,
            Some(Family::Sonnet) => self.builder,
            Some(_) => self.lead,
            None => self.muted,
        }
    }

    /// The gauge gradient at `t` (0..1).
    pub fn gauge_at(&self, t: f64) -> Color {
        gradient(&self.gauge, t)
    }

    /// The chart gradient at `t` (0..1).
    pub fn chart_at(&self, t: f64) -> Color {
        gradient(&self.chart, t)
    }

    /// Five heatmap levels, from empty to busiest.
    pub fn heat(&self) -> [Color; 5] {
        [
            self.dim,
            mix(self.dim, self.ok, 0.35),
            mix(self.dim, self.ok, 0.6),
            mix(self.dim, self.ok, 0.82),
            self.ok,
        ]
    }
}

/// `a` blended toward `b` by `t` (0..1). Non-RGB colours snap at the middle.
pub fn mix(a: Color, b: Color, t: f64) -> Color {
    let t = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.0
    };
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let ch =
                |x: u8, y: u8| (f64::from(x) + (f64::from(y) - f64::from(x)) * t).round() as u8;
            Color::Rgb(ch(r1, r2), ch(g1, g2), ch(b1, b2))
        }
        _ if t < 0.5 => a,
        _ => b,
    }
}

/// The colour at `t` (0..1) along evenly spaced `stops`.
pub fn gradient(stops: &[Color], t: f64) -> Color {
    match stops {
        [] => Color::Reset,
        [only] => *only,
        _ => {
            let t = if t.is_finite() {
                t.clamp(0.0, 1.0)
            } else {
                0.0
            };
            let scaled = t * (stops.len() - 1) as f64;
            let i = (scaled.floor() as usize).min(stops.len() - 2);
            mix(stops[i], stops[i + 1], scaled - i as f64)
        }
    }
}

/// How many colours the terminal shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorDepth {
    #[default]
    TrueColor,
    /// The xterm 256-colour palette.
    Ansi256,
}

impl ColorDepth {
    /// Truecolor when `COLORTERM` says so (or inside Windows Terminal),
    /// otherwise the 256-colour palette.
    pub fn detect(colorterm: Option<&str>, windows_terminal: bool) -> ColorDepth {
        match colorterm.map(str::to_ascii_lowercase).as_deref() {
            Some("truecolor" | "24bit") => ColorDepth::TrueColor,
            _ if windows_terminal => ColorDepth::TrueColor,
            _ => ColorDepth::Ansi256,
        }
    }

    /// Reads the environment.
    pub fn from_env() -> ColorDepth {
        let colorterm = std::env::var("COLORTERM").ok();
        ColorDepth::detect(
            colorterm.as_deref(),
            std::env::var_os("WT_SESSION").is_some(),
        )
    }

    /// Rewrites every RGB colour in `buf` to its nearest palette entry.
    pub fn apply(self, buf: &mut Buffer) {
        if self == ColorDepth::TrueColor {
            return;
        }
        for cell in &mut buf.content {
            cell.fg = to_256(cell.fg);
            cell.bg = to_256(cell.bg);
        }
    }
}

/// The nearest xterm-256 colour to an RGB one; other colours are kept.
pub fn to_256(color: Color) -> Color {
    let Color::Rgb(r, g, b) = color else {
        return color;
    };
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let nearest = |v: u8| -> usize {
        LEVELS
            .iter()
            .enumerate()
            .min_by_key(|(_, l)| (i32::from(**l) - i32::from(v)).abs())
            .map_or(0, |(i, _)| i)
    };
    let (ri, gi, bi) = (nearest(r), nearest(g), nearest(b));
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let dist = |(x, y, z): (u8, u8, u8)| {
        let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
        d(x, r) + d(y, g) + d(z, b)
    };
    // The 24 greys: 8, 18, …, 238.
    let avg = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
    let gi_ = (avg.saturating_sub(3) / 10).min(23) as u8;
    let grey = 8 + 10 * gi_;
    if dist((grey, grey, grey)) < dist(cube) {
        Color::Indexed(232 + gi_)
    } else {
        Color::Indexed(16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_is_the_default_and_keeps_the_dashboard_colours() {
        let t = Theme::default();
        assert_eq!(t.name, "reference");
        assert_eq!(t.lead, Color::Rgb(232, 140, 70));
        assert_eq!(t.haiku, Color::Rgb(110, 160, 235));
        assert_eq!(t.guard, Color::Rgb(110, 200, 130));
        assert_eq!(t.error, Color::Rgb(230, 90, 90));
        assert_eq!(t.label, Color::Rgb(150, 140, 180));
        assert_eq!(t.bg, Color::Rgb(0, 0, 0));
        assert_eq!(t.panel_bg, Color::Rgb(0, 0, 0));
        assert_eq!(to_256(t.bg), Color::Indexed(16), "true black, not 0");
    }

    #[test]
    fn themes_by_name_and_cycle() {
        assert_eq!(Theme::named("Tokyo_Night").unwrap().name, "tokyo-night");
        assert_eq!(Theme::named("nope"), None);
        let mut t = Theme::default();
        let mut seen = Vec::new();
        for _ in 0..THEMES.len() {
            seen.push(t.name);
            t = t.next();
        }
        assert_eq!(t.name, "reference", "wraps around");
        assert_eq!(seen.len(), 7);
        for name in [
            "catppuccin-mocha",
            "tokyo-night",
            "gruvbox-dark",
            "rose-pine",
            "nord",
            "dracula",
        ] {
            assert!(seen.contains(&name), "{name}");
        }
    }

    #[test]
    fn official_palette_spot_checks() {
        assert_eq!(CATPPUCCIN_MOCHA.panel_bg, Color::Rgb(0x1e, 0x1e, 0x2e));
        assert_eq!(CATPPUCCIN_MOCHA.lead, Color::Rgb(0xfa, 0xb3, 0x87));
        assert_eq!(TOKYO_NIGHT.panel_bg, Color::Rgb(0x1a, 0x1b, 0x26));
        assert_eq!(GRUVBOX_DARK.lead, Color::Rgb(0xfe, 0x80, 0x19));
        assert_eq!(ROSE_PINE.error, Color::Rgb(0xeb, 0x6f, 0x92));
        assert_eq!(NORD.bg, Color::Rgb(0x2e, 0x34, 0x40));
        assert_eq!(DRACULA.border_focus, Color::Rgb(0xbd, 0x93, 0xf9));
    }

    #[test]
    fn model_accents() {
        let t = REFERENCE;
        assert_eq!(t.model(Some("claude-haiku-4-5")), t.haiku);
        assert_eq!(t.model(Some("sonnet")), t.builder);
        assert_eq!(t.model(Some("opus")), t.lead);
        assert_eq!(t.model(None), t.muted);
        assert_ne!(
            CATPPUCCIN_MOCHA.model(Some("sonnet")),
            CATPPUCCIN_MOCHA.lead
        );
    }

    #[test]
    fn gradients_interpolate_between_stops() {
        let stops = [
            Color::Rgb(0, 0, 0),
            Color::Rgb(200, 100, 0),
            Color::Rgb(200, 200, 200),
        ];
        assert_eq!(gradient(&stops, 0.0), Color::Rgb(0, 0, 0));
        assert_eq!(gradient(&stops, 0.25), Color::Rgb(100, 50, 0));
        assert_eq!(gradient(&stops, 0.5), Color::Rgb(200, 100, 0));
        assert_eq!(gradient(&stops, 1.0), Color::Rgb(200, 200, 200));
        assert_eq!(gradient(&stops, 7.0), Color::Rgb(200, 200, 200));
        assert_eq!(gradient(&stops, f64::NAN), Color::Rgb(0, 0, 0));
        assert_eq!(gradient(&[], 0.3), Color::Reset);
        assert_eq!(mix(Color::Red, Color::Blue, 0.7), Color::Blue);
    }

    #[test]
    fn colour_depth_detection_and_fallback() {
        assert_eq!(
            ColorDepth::detect(Some("truecolor"), false),
            ColorDepth::TrueColor
        );
        assert_eq!(
            ColorDepth::detect(Some("24bit"), false),
            ColorDepth::TrueColor
        );
        assert_eq!(ColorDepth::detect(None, false), ColorDepth::Ansi256);
        assert_eq!(ColorDepth::detect(Some("yes"), true), ColorDepth::TrueColor);
        assert_eq!(to_256(Color::Rgb(255, 0, 0)), Color::Indexed(196));
        assert_eq!(to_256(Color::Rgb(0, 0, 0)), Color::Indexed(16));
        assert_eq!(to_256(Color::Rgb(128, 128, 128)), Color::Indexed(244));
        assert_eq!(to_256(Color::Green), Color::Green);
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 2, 1));
        buf[(0, 0)].fg = Color::Rgb(232, 140, 70);
        ColorDepth::Ansi256.apply(&mut buf);
        assert!(matches!(buf[(0, 0)].fg, Color::Indexed(_)));
    }

    #[test]
    fn heat_levels_rise_toward_ok() {
        let h = REFERENCE.heat();
        assert_eq!(h[0], REFERENCE.dim);
        assert_eq!(h[4], REFERENCE.ok);
    }
}
