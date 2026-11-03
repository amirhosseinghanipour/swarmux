//! The start-up screen: a neofetch-style card with the logo in the theme's
//! gradient on the left and facts about this run on the right. It goes
//! away after a moment or on any key.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::app::App;
use crate::theme::Theme;
use crate::ui::dashboard::{dim, fit, label, text};
use crate::ui::widgets::bar_spans;

/// What the splash says about this run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SplashInfo {
    /// The Claude Code executable.
    pub claude: String,
    pub sessions: usize,
    pub jobs_today: usize,
}

const LOGO: [&str; 6] = [
    "███████╗██╗    ██╗ █████╗ ██████╗ ███╗   ███╗██╗   ██╗██╗  ██╗",
    "██╔════╝██║    ██║██╔══██╗██╔══██╗████╗ ████║██║   ██║╚██╗██╔╝",
    "███████╗██║ █╗ ██║███████║██████╔╝██╔████╔██║██║   ██║ ╚███╔╝ ",
    "╚════██║██║███╗██║██╔══██║██╔══██╗██║╚██╔╝██║██║   ██║ ██╔██╗ ",
    "███████║╚███╔███╔╝██║  ██║██║  ██║██║ ╚═╝ ██║╚██████╔╝██╔╝ ██╗",
    "╚══════╝ ╚══╝╚══╝ ╚═╝  ╚═╝╚═╝  ╚═╝╚═╝     ╚═╝ ╚═════╝ ╚═╝  ╚═╝",
];

const SMALL_LOGO: [&str; 3] = [
    "┏━┓╻ ╻┏━┓┏━┓┏┳┓╻ ╻╻ ╻",
    "┗━┓┃╻┃┣━┫┣┳┛┃┃┃┃ ┃┏╋┛",
    "┗━┛┗┻┛╹ ╹╹┗╸╹ ╹┗━┛╹ ╹",
];

/// Width of the info column.
const INFO_W: u16 = 46;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let logo: &[&str] = if area.width >= LOGO_W + INFO_W + 4 && area.height >= 14 {
        &LOGO
    } else {
        &SMALL_LOGO
    };
    let logo_w = logo.first().map_or(0, |l| l.chars().count()) as u16;
    let info = info_lines(app, t);
    let info_h = info.len() as u16;
    let side_by_side = area.width >= logo_w + INFO_W + 4;
    let (w, h) = if side_by_side {
        (logo_w + 4 + INFO_W, info_h.max(logo.len() as u16))
    } else {
        (logo_w.max(INFO_W), logo.len() as u16 + 1 + info_h)
    };
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let logo_y = if side_by_side {
        y + h.saturating_sub(logo.len() as u16) / 2
    } else {
        y
    };
    for (i, row) in logo.iter().enumerate() {
        let line = gradient_line(row, t, i, logo.len());
        let rect = Rect::new(x, logo_y + i as u16, logo_w, 1).intersection(area);
        frame.render_widget(line, rect);
    }
    let (ix, iy) = if side_by_side {
        (x + logo_w + 4, y)
    } else {
        (x, y + logo.len() as u16 + 1)
    };
    for (i, line) in info.into_iter().enumerate() {
        let rect = Rect::new(ix, iy + i as u16, INFO_W, 1).intersection(area);
        frame.render_widget(line, rect);
    }
}

const LOGO_W: u16 = 62;

/// A logo row coloured along the chart gradient, left to right, with a
/// slight vertical shift so the logo reads as one sweep.
fn gradient_line(row: &str, t: &Theme, i: usize, rows: usize) -> Line<'static> {
    let n = row.chars().count().max(1);
    let lift = i as f64 / rows.max(1) as f64 * 0.15;
    Line::from(
        row.chars()
            .enumerate()
            .map(|(x, c)| {
                let at = (x as f64 / n as f64 * 0.85 + lift).min(1.0);
                let color = if c == '█' { t.chart_at(at) } else { t.dim };
                Span::styled(c.to_string(), Style::new().fg(color))
            })
            .collect::<Vec<_>>(),
    )
}

fn info_lines(app: &App, t: &Theme) -> Vec<Line<'static>> {
    let look = &app.look;
    let s = &app.splash;
    let row = |k: &str, v: String| {
        Line::from(vec![
            Span::styled(
                format!("{k:<11}"),
                Style::new().fg(t.lead).add_modifier(Modifier::BOLD),
            ),
            text(t, fit(&v, (INFO_W - 11) as usize)),
        ])
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                "swarmux",
                Style::new().fg(t.title).add_modifier(Modifier::BOLD),
            ),
            label(t, "@"),
            Span::styled(
                "claude-code",
                Style::new().fg(t.haiku).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(dim(t, "─".repeat(26))),
        row("version", env!("CARGO_PKG_VERSION").to_string()),
        row("theme", t.name.to_string()),
        row("layout", look.layout_spec().name.clone()),
        row("sessions", s.sessions.to_string()),
        row("jobs today", s.jobs_today.to_string()),
        row(
            "claude",
            if s.claude.is_empty() {
                "claude".into()
            } else {
                s.claude.clone()
            },
        ),
        row(
            "frames",
            format!(
                "{} fps{}",
                look.fps(),
                if look.animated() { "" } else { " · low power" }
            ),
        ),
        row("keys", "t theme · L layout · tab focus".into()),
        row("", "p prompt · 1-5 screens · q quit".into()),
        Line::raw(""),
    ];
    // The palette, neofetch style.
    let swatch = [
        t.lead, t.builder, t.haiku, t.guard, t.reviewer, t.ok, t.warn, t.error,
    ];
    lines.push(Line::from(
        swatch
            .iter()
            .map(|c| Span::styled("███", Style::new().fg(*c)))
            .collect::<Vec<_>>(),
    ));
    // Time left before the splash goes away.
    let left = look.splash_until.map_or(0.0, |end| {
        let total = look.frames(1500) as f64;
        (end.saturating_sub(app.frame) as f64 / total).clamp(0.0, 1.0)
    });
    let mut bar = bar_spans(left, 24, &t.chart, t.dim);
    bar.push(dim(t, "  any key"));
    lines.push(Line::from(bar));
    lines
}

#[cfg(test)]
mod tests {
    use crate::app::Launch;
    use crate::app::tests_support::test_app;
    use crate::ui::render_to_string;
    use crossterm::event::KeyCode;
    use swarmux_application::UiSettings;

    fn with_splash() -> crate::app::tests_support::TestApp {
        let mut t = test_app();
        t.with_session("demo");
        let app = std::mem::replace(&mut t.app, test_app().app);
        t.app = app.with_look(
            &UiSettings::default(),
            Launch {
                claude: "/usr/bin/claude".into(),
                no_splash: false,
                now_ms: 0,
            },
        );
        t
    }

    #[test]
    fn splash_shows_logo_and_facts_then_goes_away() {
        let mut t = with_splash();
        let out = render_to_string(&t.app, 120, 36);
        if let Ok(path) = std::env::var("SWARMUX_SPLASH_SNAPSHOT") {
            std::fs::write(path, &out).unwrap();
        }
        for needle in [
            "███████╗██╗    ██╗",
            "version    0.1.0",
            "theme      reference",
            "layout     command",
            "sessions   1",
            "claude     /usr/bin/claude",
            "t theme · L layout",
            "any key",
        ] {
            assert!(out.contains(needle), "missing {needle:?}\n{out}");
        }
        assert!(!out.contains("SWARMUX ·"), "the dashboard waits");
        t.app.frame = 15;
        let out = render_to_string(&t.app, 120, 36);
        assert!(
            !out.contains("any key"),
            "auto-dismissed after 1.5 s\n{out}"
        );
    }

    #[test]
    fn any_key_dismisses_the_splash_and_does_nothing_else() {
        let mut t = with_splash();
        t.keys([KeyCode::Char('q')]);
        assert!(!t.app.quit);
        assert!(!render_to_string(&t.app, 120, 36).contains("any key"));
    }

    #[test]
    fn splash_can_be_turned_off() {
        let t = test_app();
        let app = t.app.with_look(
            &UiSettings {
                splash: false,
                ..UiSettings::default()
            },
            Launch::default(),
        );
        assert!(!app.look.splash_at(0));
        let t = test_app();
        let app = t.app.with_look(
            &UiSettings::default(),
            Launch {
                no_splash: true,
                ..Launch::default()
            },
        );
        assert!(!app.look.splash_at(0));
    }

    #[test]
    fn splash_fits_small_windows() {
        let t = with_splash();
        for (w, h) in [(20, 5), (60, 20), (90, 30), (260, 80)] {
            render_to_string(&t.app, w, h);
        }
        assert!(render_to_string(&t.app, 80, 24).contains("┏━┓"));
    }
}
