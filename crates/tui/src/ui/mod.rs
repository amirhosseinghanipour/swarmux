//! Rendering. Reads `App` and does no I/O; the only state it touches is the
//! editor's scroll offset.

pub mod dashboard;
pub mod editor;
pub mod guard;
pub mod history;
pub mod input;
pub mod prompts;
pub mod queue;
pub mod sessions;
pub mod widgets;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Block,
};
use swarmux_application::QueueStats;
use swarmux_domain::JobStatus;

use crate::app::{App, InputPurpose, Mode, Screen};

pub const ORANGE: Color = Color::Rgb(232, 140, 70);
pub const BLUE: Color = Color::Rgb(110, 160, 235);
pub const GREEN: Color = Color::Rgb(110, 200, 130);
pub const RED: Color = Color::Rgb(230, 90, 90);
pub const MUTED: Color = Color::Rgb(120, 120, 140);
/// Labels: a muted grey-purple.
pub const LABEL: Color = Color::Rgb(150, 140, 180);
/// Placeholders and empty tracks.
pub const DIM: Color = Color::Rgb(75, 72, 90);
/// The near-black background.
pub const BG: Color = Color::Rgb(14, 13, 18);

pub fn draw(frame: &mut Frame, app: &App) {
    frame.render_widget(Block::new().style(Style::new().bg(BG)), frame.area());
    let [header, tabs, body, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let title = if app.screen == Screen::Dashboard {
        dashboard::header::title(app, &dashboard::View::of(app))
    } else {
        Line::from(vec![
            Span::styled("SWARMUX", Style::new().add_modifier(Modifier::BOLD)),
            Span::styled("  ·  claude code command center", Style::new().fg(ORANGE)),
        ])
        .centered()
    };
    frame.render_widget(title, header);
    draw_tabs(frame, tabs, app.screen);

    match app.screen {
        Screen::Dashboard => dashboard::render(frame, body, app),
        Screen::Prompts => prompts::render(frame, body, app),
        Screen::Queue => queue::render(frame, body, app),
        Screen::History => history::render(frame, body, app),
        Screen::Sessions => sessions::render(frame, body, app),
    }
    if app.guard.jev().is_some_and(|j| j.wants_consent()) {
        let h = guard::CONSENT_HEIGHT.min(body.height);
        let overlay = Rect {
            y: body.y + body.height - h,
            height: h,
            ..body
        };
        guard::render_consent(frame, overlay, app);
    } else if app.guard.pending_count() > 0 {
        let h = guard::APPROVAL_HEIGHT.min(body.height);
        let overlay = Rect {
            y: body.y + body.height - h,
            height: h,
            ..body
        };
        guard::render_approval(frame, overlay, app);
    }

    let stats = stats_line(
        app.stats(),
        app.guard.pending_count(),
        app.routing.in_flight(),
    );
    let [left, right] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(stats.width() as u16),
    ])
    .areas(status);
    draw_status(frame, left, app);
    frame.render_widget(stats, right);
}

/// A job status as a coloured word.
pub fn status_span(status: JobStatus) -> Span<'static> {
    let color = match status {
        JobStatus::Running => ORANGE,
        JobStatus::Done => GREEN,
        JobStatus::Failed | JobStatus::Interrupted => RED,
        JobStatus::Queued | JobStatus::Cancelled => MUTED,
    };
    Span::styled(status.as_str(), Style::new().fg(color))
}

/// 1234 → "1.2k", 1_000_000 → "1.0M".
pub fn short(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

/// `approvals k · routing r · running n/cap · queued m · RAM free x.x GB`,
/// right of the status bar.
fn stats_line(s: QueueStats, approvals: usize, routing: usize) -> Line<'static> {
    let ram = match s.free_ram {
        Some(b) => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        None => "n/a".into(),
    };
    let mut spans = Vec::new();
    if approvals > 0 {
        spans.push(Span::styled(
            format!("approvals {approvals}"),
            Style::new().fg(Color::Black).bg(ORANGE),
        ));
        spans.push(Span::raw(" "));
    }
    if routing > 0 {
        spans.push(Span::styled(
            format!("routing {routing} · "),
            Style::new().fg(BLUE),
        ));
    }
    spans.push(Span::styled(
        format!(
            "running {}/{} · queued {} · ",
            s.running, s.max_concurrent, s.queued
        ),
        Style::new().fg(MUTED),
    ));
    if s.ram_low {
        spans.push(Span::styled(
            format!("RAM free {ram} (low) "),
            Style::new().fg(RED),
        ));
    } else {
        spans.push(Span::styled(
            format!("RAM free {ram} "),
            Style::new().fg(MUTED),
        ));
    }
    Line::from(spans)
}

fn draw_tabs(frame: &mut Frame, area: Rect, active: Screen) {
    let spans: Vec<Span> = Screen::ALL
        .iter()
        .enumerate()
        .flat_map(|(i, s)| {
            let label = format!(" {} {} ", i + 1, s.title());
            let span = if *s == active {
                Span::styled(label, Style::new().fg(Color::Black).bg(ORANGE))
            } else {
                Span::styled(label, Style::new().fg(MUTED))
            };
            [span, Span::raw(" ")]
        })
        .collect();
    frame.render_widget(Line::from(spans), area);
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let line = match &app.mode {
        Mode::Input(purpose, input) => {
            let prompt = match purpose {
                InputPurpose::NewSession => "session name".to_string(),
                InputPurpose::Prompt => format!(
                    "prompt → {}",
                    app.selected_session().map_or("?", |s| s.name.as_str())
                ),
                InputPurpose::RoutedPrompt => "prompt → swarmux picks the session".to_string(),
                InputPurpose::Variable => format!(
                    "var {}",
                    app.prompts
                        .pending
                        .as_ref()
                        .and_then(|p| p.asking())
                        .unwrap_or("?")
                ),
                InputPurpose::Search => "search".to_string(),
            };
            let keys = if *purpose == InputPurpose::Search {
                "   enter keep · esc clear"
            } else {
                "   enter send · esc cancel"
            };
            let (before, at, after) = input.split_at_cursor();
            Line::from(vec![
                Span::styled(format!(" {prompt}: "), Style::new().fg(ORANGE)),
                Span::raw(before),
                Span::styled(at, Style::new().add_modifier(Modifier::REVERSED)),
                Span::raw(after),
                Span::styled(keys, Style::new().fg(MUTED)),
            ])
        }
        Mode::Normal => match (&app.error, app.prompts.selected_prompt()) {
            (Some(e), _) => Line::from(Span::styled(format!(" ! {e}"), Style::new().fg(RED))),
            (None, _) if app.notice.is_some() => Line::from(Span::styled(
                format!(" {}", app.notice.as_deref().unwrap_or_default()),
                Style::new().fg(GREEN),
            )),
            (None, Some(p)) if app.prompts.confirm_delete => Line::from(Span::styled(
                format!(
                    " press x again to delete \"{}\" · any other key keeps it",
                    p.name
                ),
                Style::new().fg(ORANGE),
            )),
            _ if app.screen == Screen::Dashboard => {
                dashboard::status::line(app, &dashboard::View::of(app))
            }
            _ => Line::from(Span::styled(hints(app), Style::new().fg(MUTED))),
        },
    };
    frame.render_widget(line, area);
}

pub(crate) fn hints(app: &App) -> &'static str {
    if app.prompts.form.is_some() {
        return " tab next field · ctrl+s save · esc cancel";
    }
    match app.screen {
        Screen::Dashboard => " p prompt · P route · v review · ↑↓ session · 1-5 screens · q quit",
        Screen::Queue => " c cancel · r retry · +/- priority · enter view · ↑↓ select · q quit",
        Screen::History => " / search · r re-run · f fork · enter view · ↑↓ select · q quit",
        Screen::Prompts => {
            " n new · e edit · x delete · d dispatch · D route · ↑↓ select · 1-5 screens · q quit"
        }
        Screen::Sessions => " n new session · l lean on/off · ↑↓ select · 1-5 screens · q quit",
    }
}

#[cfg(test)]
pub(crate) fn render_to_string(app: &App, w: u16, h: u16) -> String {
    use ratatui::{Terminal, backend::TestBackend};
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| draw(f, app)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests_support::test_app;

    #[test]
    fn renders_header_and_active_tab() {
        let mut t = test_app();
        t.app.screen = Screen::Queue;
        let out = render_to_string(&t.app, 80, 10);
        assert!(out.contains("SWARMUX"));
        assert!(out.contains("3 queue"));
    }

    #[test]
    fn status_bar_shows_queue_and_ram() {
        let mut t = test_app();
        let out = render_to_string(&t.app, 120, 6);
        assert!(
            out.contains("running 0/3 · queued 0 · RAM free n/a"),
            "{out}"
        );
        t.monitor.set(Some(3 << 29));
        t.with_session("demo");
        t.send("hi");
        let out = render_to_string(&t.app, 120, 6);
        assert!(
            out.contains("running 1/3 · queued 0 · RAM free 1.5 GB"),
            "{out}"
        );
    }

    #[test]
    fn low_ram_is_flagged() {
        let mut t = test_app();
        t.monitor.set(Some(1 << 29));
        t.with_session("demo");
        t.send("hi");
        assert_eq!(t.runner.started(), 0);
        let out = render_to_string(&t.app, 120, 6);
        assert!(out.contains("queued 1 · RAM free 0.5 GB (low)"), "{out}");
    }

    #[test]
    fn short_numbers() {
        assert_eq!(short(999), "999");
        assert_eq!(short(15_100), "15.1k");
        assert_eq!(short(1_000_000), "1.0M");
    }
}
