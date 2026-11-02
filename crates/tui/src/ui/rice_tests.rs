//! Themes, layouts, the telemetry panels and the black background, rendered
//! end to end.

use crossterm::event::KeyCode;
use ratatui::style::Color;

use super::{render_buffer, render_to_string};
use crate::app::App;
use crate::app::tests_support::{TestApp, test_app};
use crate::telemetry::sample;
use crate::theme::{ColorDepth, THEMES, Theme};
use crate::ui::dashboard::tests::{reviewed_job, running_job};
use crate::ui::look::IconSet;

const LAYOUTS: [&str; 3] = ["command", "btop", "minimal"];

fn set_layout(app: &mut App, name: &str) {
    app.look.layout = app
        .look
        .layouts
        .iter()
        .position(|l| l.name == name)
        .unwrap();
}

/// A reviewed job with every telemetry panel fed.
fn riced() -> TestApp {
    let mut t = reviewed_job();
    t.app.set_telemetry(sample::telemetry());
    t
}

/// Cells whose background is not the theme's: only chips and highlights may.
fn off_background(buf: &ratatui::buffer::Buffer, t: &Theme) -> Vec<(u16, u16, Color)> {
    let allowed = [t.bg, t.panel_bg, t.lead, t.warn, t.border_focus];
    let mut out = Vec::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let bg = buf[(x, y)].bg;
            if !allowed.contains(&bg) {
                out.push((x, y, bg));
            }
        }
    }
    out
}

#[test]
fn reference_paints_every_cell_pitch_black() {
    let mut t = riced();
    let black = Color::Rgb(0, 0, 0);
    for layout in LAYOUTS {
        set_layout(&mut t.app, layout);
        for (w, h) in [(80, 24), (120, 36), (160, 48), (200, 60)] {
            let buf = render_buffer(&t.app, w, h);
            let theme = &t.app.look.theme;
            assert_eq!(theme.bg, black);
            let off = off_background(&buf, theme);
            assert!(
                off.is_empty(),
                "{layout} {w}x{h}: {:?}",
                &off[..off.len().min(5)]
            );
            // Everything that isn't a chip is black: gaps, panel insides, status bar.
            let black_cells = buf.content.iter().filter(|c| c.bg == black).count();
            assert!(black_cells * 10 > buf.content.len() * 9, "{layout} {w}x{h}");
            assert_eq!(buf[(0, h - 1)].bg, black, "status bar");
        }
    }
}

#[test]
fn overlays_screens_and_splash_keep_the_background() {
    let mut t = test_app();
    t.with_session("demo");
    t.send("hi");
    let job = t.app.current_job().unwrap().job_id;
    t.inbox.push(Some(job), "Bash", "make deploy");
    t.app.update();
    for screen in crate::app::Screen::ALL {
        t.app.screen = screen;
        let buf = render_buffer(&t.app, 120, 30);
        let off = off_background(&buf, &t.app.look.theme);
        assert!(off.is_empty(), "{screen:?}: {:?}", &off[..off.len().min(5)]);
    }
    t.app.look.splash_until = Some(10);
    let buf = render_buffer(&t.app, 120, 36);
    assert!(off_background(&buf, &t.app.look.theme).is_empty());
}

#[test]
fn every_theme_paints_its_own_background() {
    let mut t = riced();
    set_layout(&mut t.app, "btop");
    for theme in THEMES {
        t.app.look.theme = theme;
        let buf = render_buffer(&t.app, 160, 48);
        let off = off_background(&buf, &theme);
        assert!(
            off.is_empty(),
            "{}: {:?}",
            theme.name,
            &off[..off.len().min(5)]
        );
        assert_eq!(buf[(0, 0)].bg, theme.bg, "{}", theme.name);
        // The lead's accent shows up somewhere.
        assert!(
            buf.content
                .iter()
                .any(|c| c.fg == theme.model(Some("sonnet"))),
            "{}",
            theme.name
        );
    }
}

#[test]
fn t_cycles_themes_with_a_toast() {
    let mut t = test_app();
    t.keys([KeyCode::Char('t')]);
    assert_eq!(t.app.look.theme.name, "catppuccin-mocha");
    let out = render_to_string(&t.app, 120, 10);
    assert!(out.contains("theme · catppuccin-mocha"), "{out}");
    for _ in 1..THEMES.len() {
        t.keys([KeyCode::Char('t')]);
    }
    assert_eq!(t.app.look.theme.name, "reference");
    t.app.frame += 100;
    assert!(
        !render_to_string(&t.app, 120, 10).contains("theme ·"),
        "the toast fades"
    );
}

#[test]
fn shift_l_cycles_layouts() {
    let mut t = riced();
    t.keys([KeyCode::Char('L')]);
    assert_eq!(t.app.look.layout_spec().name, "btop");
    let out = render_to_string(&t.app, 200, 60);
    assert!(out.contains("layout · btop"), "{out}");
    assert!(out.contains("usage limits"), "{out}");
    t.keys([KeyCode::Char('L')]);
    assert_eq!(t.app.look.layout_spec().name, "minimal");
    let out = render_to_string(&t.app, 120, 30);
    assert!(
        out.contains("main session") && out.contains("tail -f"),
        "{out}"
    );
    assert!(!out.contains("GUARD"), "{out}");
    t.keys([KeyCode::Char('L')]);
    assert_eq!(t.app.look.layout_spec().name, "command");
}

#[test]
fn tab_moves_the_focus_highlight() {
    let mut t = reviewed_job();
    t.keys([KeyCode::Tab]);
    let focused = t.app.focus.unwrap();
    assert!(focused.panel().boxed());
    let buf = render_buffer(&t.app, 160, 48);
    let theme = t.app.look.theme;
    assert!(
        buf.content.iter().any(|c| c.bg == theme.border_focus),
        "chip is inverted"
    );
    t.keys([KeyCode::Tab]);
    assert_ne!(t.app.focus, Some(focused));
    t.keys([KeyCode::Esc]);
    assert_eq!(t.app.focus, None);
}

#[test]
fn btop_layout_shows_braille_graphs_gauges_and_rings() {
    let t = {
        let mut t = riced();
        set_layout(&mut t.app, "btop");
        t
    };
    let out = render_to_string(&t.app, 200, 60);
    if let Ok(path) = std::env::var("SWARMUX_BTOP_SNAPSHOT") {
        std::fs::write(path, &out).unwrap();
    }
    for needle in [
        "usage limits",
        "5h window",
        "burn rate",
        "$12.84",
        "token flow",
        "context",
        "agent tree",
        "todo board",
        "tool activity",
        "files touched",
        "feat/health",
        "ci / pr",
        "system",
        "session wall",
        "activity",
        "environment",
        "model mix",
        "GUARD · fork layer",
        "REVIEWER",
        "who sees what",
        "tail -f session.log",
    ] {
        assert!(out.contains(needle), "missing {needle:?}\n{out}");
    }
    let braille = out
        .chars()
        .filter(|c| ('\u{2801}'..='\u{28ff}').contains(c))
        .count();
    assert!(braille > 50, "braille graphs: {braille}\n{out}");
    assert!(out.contains('■'), "heatmap cells");
    assert!(out.contains('▏') || out.contains('▎') || out.contains('▌') || out.contains('▊'));
    assert!(!out.contains("waiting for data"), "{out}");
}

#[test]
fn telemetry_panels_wait_for_data() {
    let mut t = reviewed_job();
    set_layout(&mut t.app, "btop");
    let out = render_to_string(&t.app, 200, 60);
    assert!(out.matches("waiting for data").count() >= 10, "{out}");
    assert!(out.contains("% usage limits"), "{out}");
    let buf = render_buffer(&t.app, 200, 60);
    let theme = t.app.look.theme;
    let dim_cells = buf
        .content
        .iter()
        .filter(|c| c.symbol() == "w" && c.fg == theme.dim)
        .count();
    assert!(dim_cells >= 10, "placeholders are dim");
}

#[test]
fn nerd_icons_and_braille_spinners() {
    let mut t = running_job();
    t.app.look.icons = IconSet::Nerd;
    t.app.frame = 3;
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains('\u{f0565}'), "guard shield\n{out}");
    assert!(
        out.contains("[⠙]") || out.contains("[⠹]") || out.contains("[⠋]"),
        "{out}"
    );
}

#[test]
fn ansi_256_fallback_leaves_no_rgb() {
    let mut t = riced();
    set_layout(&mut t.app, "btop");
    t.app.look.depth = ColorDepth::Ansi256;
    let buf = render_buffer(&t.app, 160, 48);
    assert!(
        buf.content
            .iter()
            .all(|c| !matches!(c.fg, Color::Rgb(..)) && !matches!(c.bg, Color::Rgb(..)))
    );
    assert_eq!(buf[(0, 0)].bg, Color::Indexed(16), "true black");
}

#[test]
fn every_layout_renders_at_every_size() {
    for mut t in [riced(), test_app()] {
        for name in LAYOUTS {
            set_layout(&mut t.app, name);
            for w in (20..=260u16)
                .step_by(9)
                .chain([79, 80, 99, 100, 149, 150, 260])
            {
                for h in (5..=80u16).step_by(7).chain([23, 24, 29, 30, 44, 45, 80]) {
                    render_to_string(&t.app, w, h);
                }
            }
        }
    }
}

#[test]
fn low_power_stops_animation() {
    let mut t = riced();
    t.app.look.low_power = true;
    assert_eq!(t.app.look.fps(), 4);
    set_layout(&mut t.app, "btop");
    let a = render_to_string(&t.app, 160, 48);
    t.app.set_telemetry(Default::default());
    t.app.set_telemetry(sample::telemetry());
    let b = render_to_string(&t.app, 160, 48);
    assert_eq!(a, b, "same tick, same frame");
}

/// `cargo test --release -p swarmux-tui render_cost -- --ignored --nocapture`
#[test]
#[ignore]
fn render_cost_btop_200x60() {
    use ratatui::{Terminal, backend::TestBackend};
    let mut t = riced();
    set_layout(&mut t.app, "btop");
    let mut terminal = Terminal::new(TestBackend::new(200, 60)).unwrap();
    let frames = 500;
    let start = std::time::Instant::now();
    for i in 0..frames {
        t.app.frame = i;
        terminal.draw(|f| super::draw(f, &t.app)).unwrap();
    }
    let per = start.elapsed() / frames as u32;
    println!("btop 200x60: {per:?} per frame over {frames} frames");
}
