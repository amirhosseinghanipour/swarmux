//! Colour themes: every colour the interface uses comes from a [`Theme`]
//! role, never from a literal. Themes are plain data; `t` cycles them.

use ratatui::{buffer::Buffer, style::Color};
use swarmux_application::{Base16, ThemeDef};
use swarmux_domain::Family;

/// Semantic colour roles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    /// The name used in `config.toml` and the toast.
    pub name: &'static str,
    /// `bg = "none"`: leave the background to the terminal. `bg` and
    /// `panel_bg` still hold the real colours (washes and contrast use them);
    /// paint with [`Theme::bg_paint`] and [`Theme::panel_paint`].
    pub transparent: bool,
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
    transparent: false,
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
    transparent: false,
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
    transparent: false,
    bg: rgb(0x16161e),           // bg_dark
    panel_bg: rgb(0x1a1b26),     // bg
    border: rgb(0x414868),       // terminal_black
    border_focus: rgb(0x7aa2f7), // blue
    title: rgb(0xc0caf5),        // fg
    text: rgb(0xc0caf5),
    muted: rgb(0x737aa2),    // dark5, the comment colour lifted to read
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
    transparent: false,
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
    transparent: false,
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
    transparent: false,
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
    transparent: false,
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

/// Nocturne: near-black blue-grey with a periwinkle accent.
pub const NOCTURNE: Theme = Theme {
    name: "nocturne",
    transparent: false,
    bg: rgb(0x0b0e14),
    panel_bg: rgb(0x0f131a),
    border: rgb(0x2a3140),
    border_focus: rgb(0x8aa2ff),
    title: rgb(0xe6ebf5),
    text: rgb(0xc9d1e0),
    muted: rgb(0x7d869a),
    label: rgb(0x9aa5bd),
    dim: rgb(0x252b38),
    lead: rgb(0xffb86b),
    builder: rgb(0xe6c37a),
    haiku: rgb(0x7cc4ff),
    guard: rgb(0x7fd99a),
    reviewer: rgb(0xb8a2ff),
    ok: rgb(0x7fd99a),
    warn: rgb(0xe6c37a),
    error: rgb(0xff6b7a),
    on_accent: rgb(0x0b0e14),
    gauge: [rgb(0x7fd99a), rgb(0xe6c37a), rgb(0xff6b7a)],
    chart: [rgb(0x7cc4ff), rgb(0x8aa2ff), rgb(0xb8a2ff)],
};

/// Catppuccin Latte, the light flavour. The warm accents are the official
/// hues stepped darker so they read on the light base.
pub const CATPPUCCIN_LATTE: Theme = Theme {
    name: "catppuccin-latte",
    transparent: false,
    bg: rgb(0xeff1f5), // base
    panel_bg: rgb(0xeff1f5),
    border: rgb(0xacb0be),       // surface2
    border_focus: rgb(0x8839ef), // mauve
    title: rgb(0x4c4f69),        // text
    text: rgb(0x4c4f69),
    muted: rgb(0x5c6070),    // subtext1, a touch deeper
    label: rgb(0x6c6f85),    // subtext0
    dim: rgb(0xccd0da),      // surface0
    lead: rgb(0xd9560a),     // peach, darker
    builder: rgb(0xb87710),  // yellow, darker
    haiku: rgb(0x1e66f5),    // blue
    guard: rgb(0x348a24),    // green, darker
    reviewer: rgb(0x8839ef), // mauve
    ok: rgb(0x179299),       // teal, so ok and error part for colour-blind eyes
    warn: rgb(0xb87710),
    error: rgb(0xd20f39), // red
    on_accent: rgb(0xeff1f5),
    gauge: [rgb(0x179299), rgb(0xb87710), rgb(0xd20f39)],
    chart: [rgb(0x179299), rgb(0x1e66f5), rgb(0x8839ef)],
};

/// Okabe-Ito on near-black: the eight colours chosen to stay apart for
/// every kind of colour-vision deficiency.
pub const OKABE_ITO: Theme = Theme {
    name: "okabe-ito",
    transparent: false,
    bg: rgb(0x0a0a0a),
    panel_bg: rgb(0x141414),
    border: rgb(0x595959),
    border_focus: rgb(0xe69f00), // orange
    title: rgb(0xffffff),
    text: rgb(0xf2f2f2),
    muted: rgb(0xa6a6a6),
    label: rgb(0xc4c4c4),
    dim: rgb(0x3a3a3a),
    lead: rgb(0xe69f00),     // orange
    builder: rgb(0xf0e442),  // yellow
    haiku: rgb(0x56b4e9),    // sky blue
    guard: rgb(0x009e73),    // bluish green
    reviewer: rgb(0xcc79a7), // reddish purple
    ok: rgb(0x56b4e9),
    warn: rgb(0xf0e442),
    error: rgb(0xd55e00), // vermillion
    on_accent: rgb(0x000000),
    gauge: [rgb(0x56b4e9), rgb(0xf0e442), rgb(0xd55e00)],
    chart: [rgb(0x56b4e9), rgb(0xcc79a7), rgb(0xe69f00)],
};

/// Kanagawa (wave).
pub const KANAGAWA: Theme = Theme {
    name: "kanagawa",
    transparent: false,
    bg: rgb(0x16161d),           // sumiInk0
    panel_bg: rgb(0x1f1f28),     // sumiInk3
    border: rgb(0x54546d),       // sumiInk6
    border_focus: rgb(0x7e9cd8), // crystalBlue
    title: rgb(0xdcd7ba),        // fujiWhite
    text: rgb(0xdcd7ba),
    muted: rgb(0x8a8980),    // fujiGray, lifted
    label: rgb(0xc8c093),    // oldWhite
    dim: rgb(0x363646),      // sumiInk4
    lead: rgb(0xffa066),     // surimiOrange
    builder: rgb(0xe6c384),  // carpYellow
    haiku: rgb(0x7fb4ca),    // springBlue
    guard: rgb(0x98bb6c),    // springGreen
    reviewer: rgb(0x957fb8), // oniViolet
    ok: rgb(0x98bb6c),
    warn: rgb(0xe6c384),
    error: rgb(0xe46876), // waveRed
    on_accent: rgb(0x16161d),
    gauge: [rgb(0x98bb6c), rgb(0xe6c384), rgb(0xe46876)],
    chart: [rgb(0x7fb4ca), rgb(0x7e9cd8), rgb(0x957fb8)],
};

/// Everforest (dark, medium).
pub const EVERFOREST: Theme = Theme {
    name: "everforest",
    transparent: false,
    bg: rgb(0x232a2e),           // bg_dim
    panel_bg: rgb(0x2d353b),     // bg0
    border: rgb(0x475258),       // bg3
    border_focus: rgb(0xa7c080), // green
    title: rgb(0xd3c6aa),        // fg
    text: rgb(0xd3c6aa),
    muted: rgb(0x859289),    // grey1
    label: rgb(0x9da9a0),    // grey2
    dim: rgb(0x3d484d),      // bg2
    lead: rgb(0xe69875),     // orange
    builder: rgb(0xdbbc7f),  // yellow
    haiku: rgb(0x7fbbb3),    // blue
    guard: rgb(0xa7c080),    // green
    reviewer: rgb(0xd699b6), // purple
    ok: rgb(0xa7c080),
    warn: rgb(0xdbbc7f),
    error: rgb(0xe67e80), // red
    on_accent: rgb(0x232a2e),
    gauge: [rgb(0xa7c080), rgb(0xdbbc7f), rgb(0xe67e80)],
    chart: [rgb(0x83c092), rgb(0x7fbbb3), rgb(0xd699b6)],
};

/// Every theme, in the order `t` cycles them.
pub const THEMES: [Theme; 12] = [
    REFERENCE,
    CATPPUCCIN_MOCHA,
    TOKYO_NIGHT,
    GRUVBOX_DARK,
    ROSE_PINE,
    NORD,
    DRACULA,
    NOCTURNE,
    CATPPUCCIN_LATTE,
    OKABE_ITO,
    KANAGAWA,
    EVERFOREST,
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
        if want == "oled" {
            return Some(REFERENCE);
        }
        THEMES.into_iter().find(|t| t.name == want)
    }

    /// The theme called `name`: one of the user's definitions (over its
    /// `base`), else a built-in.
    pub fn resolve(name: &str, defs: &[ThemeDef]) -> Option<Theme> {
        let want = name.trim();
        if let Some(def) = defs.iter().find(|d| d.name.eq_ignore_ascii_case(want)) {
            let base = def
                .base
                .as_deref()
                .and_then(Theme::named)
                .unwrap_or_default();
            return Some(Theme::from_def(def, base));
        }
        Theme::named(want)
    }

    /// This theme with the background left to the terminal (or not).
    pub fn with_transparent(mut self, transparent: bool) -> Theme {
        self.transparent = transparent;
        self
    }

    /// The colour to paint the screen background with.
    pub fn bg_paint(&self) -> Color {
        if self.transparent {
            Color::Reset
        } else {
            self.bg
        }
    }

    /// The colour to paint inside panels with.
    pub fn panel_paint(&self) -> Color {
        if self.transparent {
            Color::Reset
        } else {
            self.panel_bg
        }
    }

    /// Index of this theme in [`THEMES`].
    pub fn index(&self) -> usize {
        THEMES
            .iter()
            .position(|t| t.name == self.name)
            .unwrap_or(THEMES.len() - 1)
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

/// A name that lives for the whole run: user themes are named once, when the
/// settings load, so the set stays as small as the config.
fn intern(name: &str) -> &'static str {
    use std::sync::{Mutex, OnceLock};
    static NAMES: OnceLock<Mutex<Vec<&'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(known) = names.iter().find(|n| **n == name) {
        return known;
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    names.push(leaked);
    leaked
}

impl Theme {
    /// A theme from a base16 palette (pywal, base16 YAML, matugen): the
    /// standard slot meanings, so any scheme reads the same way.
    pub fn from_base16(palette: &Base16) -> Theme {
        let c = |i: usize| rgb(palette.slots[i]);
        Theme {
            name: palette.origin,
            transparent: false,
            bg: c(0x0),
            panel_bg: c(0x0),
            border: c(0x3),
            border_focus: c(0xD),
            title: c(0x6),
            text: c(0x5),
            muted: c(0x4),
            label: mix(c(0x4), c(0x5), 0.5),
            dim: c(0x2),
            lead: c(0x9),
            builder: c(0xA),
            haiku: c(0xD),
            guard: c(0xB),
            reviewer: c(0xE),
            ok: c(0xB),
            warn: c(0xA),
            error: c(0x8),
            on_accent: c(0x0),
            gauge: [c(0xB), c(0xA), c(0x8)],
            chart: [c(0xC), c(0xD), c(0xE)],
        }
    }

    /// The theme a `[themes.<name>]` table describes, over `base`.
    pub fn from_def(def: &ThemeDef, base: Theme) -> Theme {
        let mut t = base;
        t.name = intern(&def.name);
        for (role, colours) in &def.roles {
            let one = colours.first().copied().map(rgb);
            let three = || -> Option<[Color; 3]> {
                match colours.as_slice() {
                    [a, b, c] => Some([rgb(*a), rgb(*b), rgb(*c)]),
                    _ => None,
                }
            };
            match (role.as_str(), one) {
                ("bg", Some(v)) => t.bg = v,
                ("panel_bg", Some(v)) => t.panel_bg = v,
                ("border", Some(v)) => t.border = v,
                ("border_focus", Some(v)) => t.border_focus = v,
                ("title", Some(v)) => t.title = v,
                ("text", Some(v)) => t.text = v,
                ("muted", Some(v)) => t.muted = v,
                ("label", Some(v)) => t.label = v,
                ("dim", Some(v)) => t.dim = v,
                ("lead", Some(v)) => t.lead = v,
                ("builder", Some(v)) => t.builder = v,
                ("haiku", Some(v)) => t.haiku = v,
                ("guard", Some(v)) => t.guard = v,
                ("reviewer", Some(v)) => t.reviewer = v,
                ("ok", Some(v)) => t.ok = v,
                ("warn", Some(v)) => t.warn = v,
                ("error", Some(v)) => t.error = v,
                ("on_accent", Some(v)) => t.on_accent = v,
                ("gauge", _) => t.gauge = three().unwrap_or(t.gauge),
                ("chart", _) => t.chart = three().unwrap_or(t.chart),
                _ => {}
            }
        }
        t
    }

    /// Every role as `0xRRGGBB`, in the order [`THEME_ROLES`] lists them.
    pub fn roles(&self) -> Vec<(&'static str, Vec<u32>)> {
        let one = |c: Color| vec![rgb_of(c)];
        let many = |cs: &[Color; 3]| cs.iter().map(|c| rgb_of(*c)).collect::<Vec<_>>();
        vec![
            ("bg", one(self.bg)),
            ("panel_bg", one(self.panel_bg)),
            ("border", one(self.border)),
            ("border_focus", one(self.border_focus)),
            ("title", one(self.title)),
            ("text", one(self.text)),
            ("muted", one(self.muted)),
            ("label", one(self.label)),
            ("dim", one(self.dim)),
            ("lead", one(self.lead)),
            ("builder", one(self.builder)),
            ("haiku", one(self.haiku)),
            ("guard", one(self.guard)),
            ("reviewer", one(self.reviewer)),
            ("ok", one(self.ok)),
            ("warn", one(self.warn)),
            ("error", one(self.error)),
            ("on_accent", one(self.on_accent)),
            ("gauge", many(&self.gauge)),
            ("chart", many(&self.chart)),
        ]
    }
}

/// `0xRRGGBB` of a colour (black for anything that isn't RGB).
pub fn rgb_of(color: Color) -> u32 {
    match color {
        Color::Rgb(r, g, b) => (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b),
        _ => 0,
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
        assert_eq!(seen.len(), THEMES.len());
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
    fn oled_is_an_alias_of_the_pure_black_default() {
        assert_eq!(Theme::named("oled"), Some(REFERENCE));
        assert_eq!(
            Theme::named("OLED").map(|t| t.bg),
            Some(Color::Rgb(0, 0, 0))
        );
        assert_eq!(Theme::default().name, "reference");
    }

    #[test]
    fn official_palette_spot_checks() {
        assert_eq!(NOCTURNE.bg, Color::Rgb(0x0b, 0x0e, 0x14));
        assert_eq!(NOCTURNE.border_focus, Color::Rgb(0x8a, 0xa2, 0xff));
        assert_eq!(CATPPUCCIN_LATTE.bg, Color::Rgb(0xef, 0xf1, 0xf5));
        assert_eq!(OKABE_ITO.lead, Color::Rgb(0xe6, 0x9f, 0x00));
        assert_eq!(OKABE_ITO.haiku, Color::Rgb(0x56, 0xb4, 0xe9));
        assert_eq!(KANAGAWA.panel_bg, Color::Rgb(0x1f, 0x1f, 0x28));
        assert_eq!(KANAGAWA.error, Color::Rgb(0xe4, 0x68, 0x76));
        assert_eq!(EVERFOREST.panel_bg, Color::Rgb(0x2d, 0x35, 0x3b));
        assert_eq!(EVERFOREST.text, Color::Rgb(0xd3, 0xc6, 0xaa));
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

    #[test]
    fn base16_palettes_map_onto_the_roles() {
        let mut slots = [0u32; 16];
        for (i, s) in slots.iter_mut().enumerate() {
            *s = 0x101010 * (i as u32 + 1);
        }
        let p = Base16 {
            origin: "wal",
            name: "x".into(),
            slots,
        };
        let t = Theme::from_base16(&p);
        assert_eq!(t.name, "wal");
        assert_eq!(t.bg, rgb(slots[0]));
        assert_eq!(t.text, rgb(slots[5]));
        assert_eq!(t.error, rgb(slots[8]));
        assert_eq!(t.ok, rgb(slots[0xB]));
        assert_eq!(t.haiku, rgb(slots[0xD]));
    }

    #[test]
    fn a_theme_survives_a_round_trip_through_its_definition() {
        for t in THEMES {
            let roles = t
                .roles()
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect();
            let def = ThemeDef {
                name: t.name.into(),
                base: None,
                roles,
            };
            assert_eq!(Theme::from_def(&def, REFERENCE), t, "{}", t.name);
        }
    }
}
