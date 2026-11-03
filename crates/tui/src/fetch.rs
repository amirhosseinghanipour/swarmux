//! `swarmux fetch`: a neofetch-style card for the terminal's scrollback.
//! The logo in the theme's gradient, key/value rows, a stacked bar and the
//! sixteen terminal colours. Pure text out: the caller gathers the facts and
//! prints the string.

use ratatui::style::Color;

use crate::theme::{Theme, mix};

/// What the card says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FetchInfo {
    pub version: String,
    pub theme: String,
    /// Sessions swarmux knows.
    pub sessions: usize,
    /// Claude Code sessions that spoke in the last two minutes.
    pub agents_running: usize,
    pub spend_today: Option<f64>,
    pub burn_per_hour: Option<f64>,
    /// Share of the 5-hour window used, `0.0..=1.0`.
    pub limit_5h: Option<f64>,
    /// Share of the 7-day window used.
    pub limit_7d: Option<f64>,
    pub missions_delivered: usize,
    /// First characters of the newest ledger entry's hash.
    pub ledger_head: Option<String>,
    pub claude: Option<String>,
    pub os: String,
    /// What the stacked bar is about, e.g. `spend today by model`.
    pub mix_title: String,
    /// `(model, share)`; shares need not add up to 1.
    pub mix: Vec<(String, f64)>,
}

/// How many colours the card may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    TrueColor,
    Ansi256,
    Ansi16,
    /// No escape sequences at all.
    Plain,
}

/// The user's say on colour: `--color`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

impl ColorChoice {
    pub fn parse(value: &str) -> Option<ColorChoice> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(ColorChoice::Auto),
            "always" => Some(ColorChoice::Always),
            "never" => Some(ColorChoice::Never),
            _ => None,
        }
    }
}

impl Depth {
    /// The depth for this terminal. `NO_COLOR` (any non-empty value) wins
    /// over everything, `--color never` too; `auto` prints plain text when
    /// stdout is not a terminal.
    pub fn detect(
        choice: ColorChoice,
        is_tty: bool,
        var: impl Fn(&str) -> Option<String>,
    ) -> Depth {
        let set = |name: &str| var(name).is_some_and(|v| !v.is_empty());
        if set("NO_COLOR") || choice == ColorChoice::Never {
            return Depth::Plain;
        }
        if choice == ColorChoice::Auto && !is_tty {
            return Depth::Plain;
        }
        let term = var("TERM").unwrap_or_default().to_ascii_lowercase();
        let colorterm = var("COLORTERM").unwrap_or_default().to_ascii_lowercase();
        if term == "dumb" {
            Depth::Plain
        } else if matches!(colorterm.as_str(), "truecolor" | "24bit") || set("WT_SESSION") {
            Depth::TrueColor
        } else if term.contains("256color") || term.contains("direct") {
            Depth::Ansi256
        } else {
            Depth::Ansi16
        }
    }

    /// Reads the process environment.
    pub fn from_env(choice: ColorChoice, is_tty: bool) -> Depth {
        Depth::detect(choice, is_tty, |name| std::env::var(name).ok())
    }
}

/// Terminal columns, when stdout is a terminal.
pub fn terminal_columns() -> Option<u16> {
    crossterm::terminal::size().ok().map(|(w, _)| w)
}

const LOGO: [&str; 6] = [
    "███████╗██╗    ██╗ █████╗ ██████╗ ███╗   ███╗██╗   ██╗██╗  ██╗",
    "██╔════╝██║    ██║██╔══██╗██╔══██╗████╗ ████║██║   ██║╚██╗██╔╝",
    "███████╗██║ █╗ ██║███████║██████╔╝██╔████╔██║██║   ██║ ╚███╔╝ ",
    "╚════██║██║███╗██║██╔══██║██╔══██╗██║╚██╔╝██║██║   ██║ ██╔██╗ ",
    "███████║╚███╔███╔╝██║  ██║██║  ██║██║ ╚═╝ ██║╚██████╔╝██╔╝ ██╗",
    "╚══════╝ ╚══╝╚══╝ ╚═╝  ╚═╝╚═╝  ╚═╝╚═╝     ╚═╝ ╚═════╝ ╚═╝  ╚═╝",
];
const LOGO_W: usize = 62;
/// Columns of the info block.
const INFO_W: usize = 44;
/// Columns between the logo and the info.
const GAP: usize = 4;
/// Width from which the logo stands left of the info.
const SIDE_BY_SIDE: usize = LOGO_W + GAP + INFO_W;
const KEY_W: usize = 9;

/// The sixteen standard colours as terminals commonly draw them, for the
/// nearest-colour match at 16-colour depth.
const ANSI16: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (205, 49, 49),
    (13, 188, 121),
    (229, 229, 16),
    (36, 114, 200),
    (188, 63, 188),
    (17, 168, 205),
    (229, 229, 229),
    (102, 102, 102),
    (241, 76, 76),
    (35, 209, 139),
    (245, 245, 67),
    (59, 142, 234),
    (214, 112, 214),
    (41, 184, 219),
    (255, 255, 255),
];

/// Writes styled text for one depth.
struct Ink {
    depth: Depth,
}

impl Ink {
    fn fg(&self, color: Color) -> String {
        let Color::Rgb(r, g, b) = color else {
            return String::new();
        };
        match self.depth {
            Depth::Plain => String::new(),
            Depth::TrueColor => format!("\x1b[38;2;{r};{g};{b}m"),
            Depth::Ansi256 => match crate::theme::to_256(color) {
                Color::Indexed(i) => format!("\x1b[38;5;{i}m"),
                _ => String::new(),
            },
            Depth::Ansi16 => {
                let i = nearest16(r, g, b);
                let code = if i < 8 { 30 + i } else { 90 + i - 8 };
                format!("\x1b[{code}m")
            }
        }
    }

    fn paint(&self, color: Color, text: &str) -> String {
        self.styled(color, false, text)
    }

    fn bold(&self, color: Color, text: &str) -> String {
        self.styled(color, true, text)
    }

    fn styled(&self, color: Color, bold: bool, text: &str) -> String {
        if self.depth == Depth::Plain || text.is_empty() {
            return text.to_string();
        }
        let weight = if bold { "\x1b[1m" } else { "" };
        format!("{weight}{}{text}\x1b[0m", self.fg(color))
    }
}

fn nearest16(r: u8, g: u8, b: u8) -> u8 {
    let dist = |(x, y, z): (u8, u8, u8)| {
        let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
        d(x, r) + d(y, g) + d(z, b)
    };
    ANSI16
        .iter()
        .enumerate()
        .min_by_key(|(_, c)| dist(**c))
        .map_or(7, |(i, _)| i as u8)
}

/// The card as text, `width` columns wide at most.
pub fn render(info: &FetchInfo, theme: &Theme, depth: Depth, width: u16) -> String {
    let ink = Ink { depth };
    let logo = logo_lines(theme, &ink);
    let info_lines = info_block(info, theme, &ink);
    let mut out = String::new();
    if usize::from(width) >= SIDE_BY_SIDE {
        let offset = info_lines.len().saturating_sub(logo.len()) / 2;
        let rows = info_lines.len().max(logo.len());
        for row in 0..rows {
            let left = row
                .checked_sub(offset)
                .and_then(|i| logo.get(i))
                .cloned()
                .unwrap_or_else(|| " ".repeat(LOGO_W));
            let right = info_lines.get(row).cloned().unwrap_or_default();
            out.push_str(&left);
            out.push_str(&" ".repeat(GAP));
            out.push_str(&right);
            out.push('\n');
        }
    } else {
        for line in &logo {
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
        for line in &info_lines {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The logo, each row coloured along the theme's chart gradient.
fn logo_lines(theme: &Theme, ink: &Ink) -> Vec<String> {
    LOGO.iter()
        .enumerate()
        .map(|(row, line)| {
            let n = line.chars().count().max(1);
            let lift = row as f64 / LOGO.len() as f64 * 0.15;
            let mut s = String::new();
            for (x, c) in line.chars().enumerate() {
                let at = (x as f64 / n as f64 * 0.85 + lift).min(1.0);
                let color = if c == '█' {
                    theme.chart_at(at)
                } else {
                    mix(theme.dim, theme.chart_at(at), 0.35)
                };
                s.push_str(&ink.paint(color, &c.to_string()));
            }
            s
        })
        .collect()
}

fn info_block(info: &FetchInfo, t: &Theme, ink: &Ink) -> Vec<String> {
    let key = |k: &str| ink.bold(t.lead, &format!("{k:<KEY_W$}"));
    let row = |k: &str, v: &str| format!("{}{}", key(k), ink.paint(t.text, v));
    let mut lines = vec![
        format!(
            "{}{}{}",
            ink.bold(t.title, "swarmux"),
            ink.paint(t.label, "@"),
            ink.bold(t.haiku, "claude-code")
        ),
        ink.paint(t.dim, &"─".repeat(INFO_W.min(26))),
        row("version", &info.version),
        row("theme", &info.theme),
        row("sessions", &info.sessions.to_string()),
        row(
            "agents",
            &if info.agents_running == 0 {
                "none running".to_string()
            } else {
                format!("{} running", info.agents_running)
            },
        ),
        row("today", &money(info.spend_today, info.burn_per_hour)),
    ];
    lines.push(format!(
        "{}{}",
        key("5h limit"),
        gauge(info.limit_5h, t, ink)
    ));
    lines.push(format!(
        "{}{}",
        key("7d limit"),
        gauge(info.limit_7d, t, ink)
    ));
    lines.push(row(
        "missions",
        &format!("{} delivered", info.missions_delivered),
    ));
    lines.push(row(
        "ledger",
        info.ledger_head.as_deref().unwrap_or("empty"),
    ));
    lines.push(row("claude", info.claude.as_deref().unwrap_or("not found")));
    lines.push(row("os", &info.os));
    if !info.mix.is_empty() {
        lines.push(String::new());
        lines.extend(stacked_bar(info, t, ink));
    }
    if ink.depth != Depth::Plain {
        lines.push(String::new());
        lines.push(palette_row());
    }
    lines
}

fn money(spend: Option<f64>, burn: Option<f64>) -> String {
    match (spend, burn) {
        (Some(s), Some(b)) => format!("${s:.2} · ${b:.2}/h"),
        (Some(s), None) => format!("${s:.2}"),
        _ => "no usage yet".to_string(),
    }
}

/// `████░░░░░░ 42%`, coloured along the gauge gradient.
fn gauge(ratio: Option<f64>, t: &Theme, ink: &Ink) -> String {
    let Some(ratio) = ratio else {
        return ink.paint(t.muted, "unknown");
    };
    let ratio = if ratio.is_finite() {
        ratio.clamp(0.0, 1.0)
    } else {
        0.0
    };
    const CELLS: usize = 12;
    let filled = (ratio * CELLS as f64).round() as usize;
    let mut s = String::new();
    for i in 0..CELLS {
        if i < filled {
            let at = i as f64 / (CELLS - 1) as f64;
            s.push_str(&ink.paint(t.gauge_at(at), "█"));
        } else {
            s.push_str(&ink.paint(t.dim, "░"));
        }
    }
    s.push_str(&ink.paint(t.text, &format!(" {:.0}%", ratio * 100.0)));
    s
}

/// A short name for a model: `claude-opus-4-5` becomes `opus`.
pub fn short_model(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    for family in ["opus", "sonnet", "haiku"] {
        if lower.contains(family) {
            return family.to_string();
        }
    }
    name.trim_start_matches("claude-").to_string()
}

/// The onefetch-style bar: one segment per model, widths by share, and a
/// legend under it.
fn stacked_bar(info: &FetchInfo, t: &Theme, ink: &Ink) -> Vec<String> {
    let total: f64 = info.mix.iter().map(|(_, s)| s.max(0.0)).sum();
    if total <= 0.0 {
        return Vec::new();
    }
    let shares: Vec<f64> = info.mix.iter().map(|(_, s)| s.max(0.0) / total).collect();
    let widths = apportion(&shares, INFO_W);
    let mut bar = String::new();
    for ((name, _), w) in info.mix.iter().zip(&widths) {
        bar.push_str(&ink.paint(t.model(Some(name)), &"█".repeat(*w)));
    }
    let mut legend = String::new();
    for ((name, _), share) in info.mix.iter().zip(&shares) {
        if !legend.is_empty() {
            legend.push_str("  ");
        }
        legend.push_str(&ink.paint(t.model(Some(name)), "●"));
        legend.push_str(&ink.paint(
            t.text,
            &format!(" {} {:.0}%", short_model(name), share * 100.0),
        ));
    }
    vec![ink.paint(t.label, &info.mix_title), bar, legend]
}

/// Splits `cells` among `shares` (summing to 1) by largest remainder, so
/// the segments always add up to the bar's width.
pub fn apportion(shares: &[f64], cells: usize) -> Vec<usize> {
    let exact: Vec<f64> = shares.iter().map(|s| s * cells as f64).collect();
    let mut widths: Vec<usize> = exact.iter().map(|e| e.floor() as usize).collect();
    let mut left = cells.saturating_sub(widths.iter().sum());
    let mut order: Vec<usize> = (0..exact.len()).collect();
    order.sort_by(|a, b| {
        let (fa, fb) = (exact[*a].fract(), exact[*b].fract());
        fb.partial_cmp(&fa).unwrap_or(std::cmp::Ordering::Equal)
    });
    for i in order {
        if left == 0 {
            break;
        }
        widths[i] += 1;
        left -= 1;
    }
    widths
}

/// The terminal's sixteen colours, one row of blocks.
fn palette_row() -> String {
    (30..38)
        .chain(90..98)
        .map(|code: u8| format!("[{code}m██[0m"))
        .collect()
}

/// The text of `s` without its escape sequences.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{NOCTURNE, REFERENCE};

    fn sample() -> FetchInfo {
        FetchInfo {
            version: "0.1.0".into(),
            theme: "nocturne".into(),
            sessions: 6,
            agents_running: 3,
            spend_today: Some(12.84),
            burn_per_hour: Some(3.2),
            limit_5h: Some(0.42),
            limit_7d: Some(0.71),
            missions_delivered: 9,
            ledger_head: Some("a1b2c3d4".into()),
            claude: Some("2.1.3".into()),
            os: "windows x86_64".into(),
            mix_title: "spend today by model".into(),
            mix: vec![
                ("claude-opus-4-5".into(), 7.1),
                ("claude-sonnet-4-5".into(), 4.6),
                ("claude-haiku-4-5".into(), 1.1),
            ],
        }
    }

    fn trimmed(text: &str) -> String {
        text.lines().map(str::trim_end).collect::<Vec<_>>().join(
            "
",
        )
    }

    #[test]
    fn the_wide_card_stands_the_logo_left_of_the_facts() {
        let card = render(&sample(), &NOCTURNE, Depth::TrueColor, 140);
        let expected = "                                                                  swarmux@claude-code
                                                                  ──────────────────────────
                                                                  version  0.1.0
                                                                  theme    nocturne
                                                                  sessions 6
                                                                  agents   3 running
███████╗██╗    ██╗ █████╗ ██████╗ ███╗   ███╗██╗   ██╗██╗  ██╗    today    $12.84 · $3.20/h
██╔════╝██║    ██║██╔══██╗██╔══██╗████╗ ████║██║   ██║╚██╗██╔╝    5h limit █████░░░░░░░ 42%
███████╗██║ █╗ ██║███████║██████╔╝██╔████╔██║██║   ██║ ╚███╔╝     7d limit █████████░░░ 71%
╚════██║██║███╗██║██╔══██║██╔══██╗██║╚██╔╝██║██║   ██║ ██╔██╗     missions 9 delivered
███████║╚███╔███╔╝██║  ██║██║  ██║██║ ╚═╝ ██║╚██████╔╝██╔╝ ██╗    ledger   a1b2c3d4
╚══════╝ ╚══╝╚══╝ ╚═╝  ╚═╝╚═╝  ╚═╝╚═╝     ╚═╝ ╚═════╝ ╚═╝  ╚═╝    claude   2.1.3
                                                                  os       windows x86_64

                                                                  spend today by model
                                                                  ████████████████████████████████████████████
                                                                  ● opus 55%  ● sonnet 36%  ● haiku 9%

                                                                  ████████████████████████████████";
        assert_eq!(trimmed(&strip_ansi(&card)), expected);
    }

    #[test]
    fn the_narrow_card_stacks_and_plain_has_no_palette() {
        let card = render(&sample(), &REFERENCE, Depth::Plain, 80);
        assert!(!card.contains(''), "no escape sequences when plain");
        let text = trimmed(&card);
        let rows: Vec<&str> = text.lines().collect();
        assert!(rows[0].starts_with("███████╗"), "logo first");
        assert_eq!(rows[6], "");
        assert_eq!(rows[7], "swarmux@claude-code");
        assert!(text.ends_with("● opus 55%  ● sonnet 36%  ● haiku 9%"));
    }

    #[test]
    fn colour_depths_use_their_own_escape_sequences() {
        let at = |depth| render(&sample(), &NOCTURNE, depth, 140);
        let truecolor = at(Depth::TrueColor);
        assert!(truecolor.contains("[38;2;"));
        let c256 = at(Depth::Ansi256);
        assert!(c256.contains("[38;5;") && !c256.contains("38;2;"));
        let c16 = at(Depth::Ansi16);
        assert!(!c16.contains("38;"), "no extended colours at 16");
        assert!(c16.contains("[9") || c16.contains("[3"));
        for card in [&truecolor, &c256, &c16] {
            assert!(
                card.contains("[90m██"),
                "the palette row is the terminal's own"
            );
            assert_eq!(strip_ansi(card).lines().count(), 19);
        }
        assert_eq!(
            strip_ansi(&truecolor),
            strip_ansi(&c16),
            "same text at every depth"
        );
    }

    #[test]
    fn depth_follows_no_color_the_flag_and_the_environment() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| (*v).to_string())
            }
        };
        let auto = ColorChoice::Auto;
        assert_eq!(
            Depth::detect(auto, true, env(&[("NO_COLOR", "1")])),
            Depth::Plain
        );
        assert_eq!(
            Depth::detect(ColorChoice::Always, true, env(&[("NO_COLOR", "1")])),
            Depth::Plain,
            "NO_COLOR beats --color always"
        );
        assert_eq!(
            Depth::detect(auto, true, env(&[("NO_COLOR", "")])),
            Depth::Ansi16
        );
        assert_eq!(Depth::detect(auto, false, env(&[])), Depth::Plain, "piped");
        assert_eq!(
            Depth::detect(
                ColorChoice::Always,
                false,
                env(&[("COLORTERM", "truecolor")])
            ),
            Depth::TrueColor
        );
        assert_eq!(
            Depth::detect(auto, true, env(&[("WT_SESSION", "x")])),
            Depth::TrueColor
        );
        assert_eq!(
            Depth::detect(auto, true, env(&[("TERM", "xterm-256color")])),
            Depth::Ansi256
        );
        assert_eq!(
            Depth::detect(auto, true, env(&[("TERM", "dumb")])),
            Depth::Plain
        );
        assert_eq!(
            Depth::detect(ColorChoice::Never, true, env(&[("COLORTERM", "truecolor")])),
            Depth::Plain
        );
        assert_eq!(ColorChoice::parse("Always"), Some(ColorChoice::Always));
        assert_eq!(ColorChoice::parse("sometimes"), None);
    }

    #[test]
    fn missing_facts_read_as_such_and_odd_numbers_do_not_break_the_bar() {
        let card = render(&FetchInfo::default(), &REFERENCE, Depth::Plain, 140);
        let text = trimmed(&card);
        assert!(text.contains("no usage yet") && text.contains("not found"));
        assert!(text.contains("5h limit unknown") && text.contains("ledger   empty"));
        for shares in [
            vec![1.0],
            vec![0.5, 0.5],
            vec![0.34, 0.33, 0.33],
            vec![0.0, 1.0],
        ] {
            let widths = apportion(&shares, 44);
            assert_eq!(widths.iter().sum::<usize>(), 44, "{shares:?}");
        }
        let weird = FetchInfo {
            limit_5h: Some(f64::NAN),
            limit_7d: Some(7.0),
            mix: vec![("x".into(), f64::NAN), ("y".into(), -1.0)],
            ..FetchInfo::default()
        };
        let _ = render(&weird, &REFERENCE, Depth::TrueColor, 10);
    }

    #[test]
    fn model_names_are_shortened() {
        assert_eq!(short_model("claude-opus-4-5"), "opus");
        assert_eq!(short_model("claude-3-5-haiku-20241022"), "haiku");
        assert_eq!(short_model("claude-mystery-1"), "mystery-1");
    }
}
