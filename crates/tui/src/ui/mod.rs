//! Rendering. Reads `App` and does no I/O; the only state it touches is the
//! editor's scroll offset.

pub mod dashboard;
pub mod editor;
pub mod history;
pub mod input;
pub mod prompts;
pub mod queue;
pub mod sessions;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use swarmux_application::QueueStats;
use swarmux_domain::JobStatus;

use crate::app::{App, InputPurpose, Mode, Screen};

pub const ORANGE: Color = Color::Rgb(232, 140, 70);
pub const BLUE: Color = Color::Rgb(110, 160, 235);
pub const GREEN: Color = Color::Rgb(110, 200, 130);
pub const RED: Color = Color::Rgb(230, 90, 90);
pub const MUTED: Color = Color::Rgb(120, 120, 140);

pub fn draw(frame: &mut Frame, app: &App) {
    let [header, tabs, body, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(
        Line::from(vec![
            Span::styled("SWARMUX", Style::new().add_modifier(Modifier::BOLD)),
            Span::styled("  ·  claude code command center", Style::new().fg(ORANGE)),
        ])
        .centered(),
        header,
    );
    draw_tabs(frame, tabs, app.screen);

    match app.screen {
        Screen::Dashboard => dashboard::render(frame, body, app),
        Screen::Prompts => prompts::render(frame, body, app),
        Screen::Queue => queue::render(frame, body, app),
        Screen::History => history::render(frame, body, app),
        Screen::Sessions => sessions::render(frame, body, app),
    }

    let stats = stats_line(app.stats());
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

/// `running n/cap · queued m · RAM free x.x GB`, right of the status bar.
fn stats_line(s: QueueStats) -> Line<'static> {
    let ram = match s.free_ram {
        Some(b) => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        None => "n/a".into(),
    };
    let mut spans = vec![Span::styled(
        format!(
            "running {}/{} · queued {} · ",
            s.running, s.max_concurrent, s.queued
        ),
        Style::new().fg(MUTED),
    )];
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
            (None, Some(p)) if app.prompts.confirm_delete => Line::from(Span::styled(
                format!(
                    " press x again to delete \"{}\" · any other key keeps it",
                    p.name
                ),
                Style::new().fg(ORANGE),
            )),
            _ => Line::from(Span::styled(hints(app), Style::new().fg(MUTED))),
        },
    };
    frame.render_widget(line, area);
}

fn hints(app: &App) -> &'static str {
    if app.prompts.form.is_some() {
        return " tab next field · ctrl+s save · esc cancel";
    }
    match app.screen {
        Screen::Dashboard => " p prompt · ↑↓ session · 3 queue · 1-5 screens · q quit",
        Screen::Queue => " c cancel · r retry · +/- priority · enter view · ↑↓ select · q quit",
        Screen::History => " / search · r re-run · f fork · enter view · ↑↓ select · q quit",
        Screen::Prompts => {
            " n new · e edit · x delete · d dispatch · ↑↓ select · 1-5 screens · q quit"
        }
        Screen::Sessions => " n new session · ↑↓ select · 1-5 screens · q quit",
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
