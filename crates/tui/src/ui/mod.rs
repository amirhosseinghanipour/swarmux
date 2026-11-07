//! Rendering. Reads `App` and does no I/O; the only state it touches is the
//! editor's scroll offset and the animation state (tweens and pulses).

pub mod anim;
pub mod board;
pub mod confirm;
pub mod constellation;
pub mod dashboard;
pub mod editor;
pub mod focus;
pub mod grid;
pub mod guard;
pub mod input;
pub mod ledger;
pub mod look;
pub mod mission;
pub mod missions;
pub mod motion;
pub mod prompts;
pub mod queue;
pub mod sessions;
mod showcase_chrome;
pub mod spectrum;
pub mod splash;
pub mod telemetry;
pub mod widgets;

#[cfg(test)]
mod rice_tests;
#[cfg(test)]
mod showcase_tests;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType},
};
use swarmux_application::QueueStats;
use swarmux_domain::JobStatus;

use crate::app::{App, InputPurpose, Mode, Screen};
use crate::theme::Theme;

pub fn draw(frame: &mut Frame, app: &App) {
    let t = &app.look.theme;
    frame.render_widget(
        Block::new().style(Style::new().bg(t.bg_paint()).fg(t.text)),
        frame.area(),
    );
    if app.look.splash_at(app.frame) {
        splash::render(frame, frame.area(), app);
    } else if let Some(mode) = app.showcase.mode {
        match mode {
            crate::showcase::Mode::Constellation => {
                constellation::render(frame, frame.area(), app);
            }
            crate::showcase::Mode::Spectrum => spectrum::render(frame, frame.area(), app),
        }
    } else {
        draw_screens(frame, app, t);
    }
    app.look.depth.apply(frame.buffer_mut());
}

fn draw_screens(frame: &mut Frame, app: &App, t: &Theme) {
    let [header, tabs, body, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let title = if app.screen == Screen::Dashboard && app.dashboard.classic {
        dashboard::header::title(app, &dashboard::View::of(app), t)
    } else {
        Line::from(vec![
            Span::styled(
                "SWARMUX",
                Style::new().fg(t.title).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  verified autonomy", Style::new().fg(t.lead)),
        ])
        .centered()
    };
    frame.render_widget(title, header);
    draw_tabs(frame, tabs, app, t);

    match app.screen {
        Screen::Mission => mission::render(frame, body, app),
        Screen::Missions => missions::render(frame, body, app),
        Screen::Dashboard if app.dashboard.classic => dashboard::render(frame, body, app),
        Screen::Dashboard => focus::render(frame, body, app),
        Screen::Prompts => prompts::render(frame, body, app),
        Screen::Queue => queue::render(frame, body, app),
        Screen::Ledger => ledger::render(frame, body, app),
        Screen::Sessions => sessions::render(frame, body, app),
        Screen::Board => board::render(frame, body, app),
    }
    if app.engine.guard.jev().is_some_and(|j| j.wants_consent()) {
        let h = guard::CONSENT_HEIGHT.min(body.height);
        let overlay = Rect {
            y: body.y + body.height - h,
            height: h,
            ..body
        };
        guard::render_consent(frame, overlay, app);
    } else if let Mode::Confirm(c) = &app.mode {
        let h = confirm::HEIGHT.min(body.height);
        let overlay = Rect {
            y: body.y + body.height - h,
            height: h,
            ..body
        };
        confirm::render(frame, overlay, app, c);
    } else if app.engine.guard.pending_count() > 0 {
        let compact = body.height < guard::COMPACT_BELOW;
        let h = if compact {
            1
        } else {
            guard::APPROVAL_HEIGHT.min(body.height)
        };
        let overlay = Rect {
            y: body.y + body.height - h,
            height: h,
            ..body
        };
        if compact {
            guard::render_approval_bar(frame, overlay, app);
        } else {
            guard::render_approval(frame, overlay, app);
        }
    }

    let stats = stats_line(
        t,
        app.stats(),
        app.engine.guard.pending_count(),
        app.routing.in_flight(),
    );
    let tag = app.demo_label().map(|label| {
        Line::from(Span::styled(
            format!(" {label} "),
            Style::new()
                .fg(t.on_accent)
                .bg(t.warn)
                .add_modifier(Modifier::BOLD),
        ))
    });
    let [left, middle, right] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(tag.as_ref().map_or(0, |l| l.width() as u16 + 1)),
        Constraint::Length(stats.width() as u16),
    ])
    .areas(status);
    draw_status(frame, left, app, t);
    if let Some(tag) = tag {
        frame.render_widget(tag, middle);
    }
    frame.render_widget(stats, right);
}

/// Rows the approval or consent overlay takes at the bottom of a body
/// `body_height` rows tall.
pub(crate) fn overlay_rows(app: &App, body_height: u16) -> u16 {
    if app.engine.guard.jev().is_some_and(|j| j.wants_consent()) {
        guard::CONSENT_HEIGHT.min(body_height)
    } else if app.engine.guard.pending_count() > 0 {
        if body_height < guard::COMPACT_BELOW {
            1
        } else {
            guard::APPROVAL_HEIGHT.min(body_height)
        }
    } else {
        0
    }
}

/// A rounded panel border in the theme's border colour.
pub fn rounded(t: &Theme) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t.border))
        .style(Style::new().bg(t.panel_paint()))
}

/// A job status as a coloured word.
pub fn status_span(t: &Theme, status: JobStatus) -> Span<'static> {
    let color = match status {
        JobStatus::Running => t.lead,
        JobStatus::Done => t.ok,
        JobStatus::Failed | JobStatus::Interrupted => t.error,
        JobStatus::Queued | JobStatus::Cancelled => t.muted,
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
fn stats_line(t: &Theme, s: QueueStats, approvals: usize, routing: usize) -> Line<'static> {
    let ram = match s.free_ram {
        Some(b) => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        None => "n/a".into(),
    };
    let mut spans = Vec::new();
    if approvals > 0 {
        spans.push(Span::styled(
            format!("approvals {approvals}"),
            Style::new().fg(t.on_accent).bg(t.warn),
        ));
        spans.push(Span::raw(" "));
    }
    if routing > 0 {
        spans.push(Span::styled(
            format!("routing {routing} · "),
            Style::new().fg(t.haiku),
        ));
    }
    spans.push(Span::styled(
        format!(
            "running {}/{} · queued {} · ",
            s.running, s.max_concurrent, s.queued
        ),
        Style::new().fg(t.muted),
    ));
    if s.deferred > 0 {
        spans.push(Span::styled(
            format!("deferred {} · ", s.deferred),
            Style::new().fg(t.warn),
        ));
    }
    if s.awaiting_reset > 0 {
        spans.push(Span::styled(
            format!("awaiting reset {} · ", s.awaiting_reset),
            Style::new().fg(t.warn),
        ));
    }
    if s.on_battery {
        spans.push(Span::styled("battery · ", Style::new().fg(t.muted)));
    }
    if s.ram_low {
        spans.push(Span::styled(
            format!("RAM free {ram} (low) "),
            Style::new().fg(t.error),
        ));
    } else {
        spans.push(Span::styled(
            format!("RAM free {ram} "),
            Style::new().fg(t.muted),
        ));
    }
    Line::from(spans)
}

/// The screens as chips, the active one filled; theme and layout on the right.
fn draw_tabs(frame: &mut Frame, area: Rect, app: &App, t: &Theme) {
    let mut spans = Vec::new();
    for s in Screen::ALL.iter() {
        let label = format!(" {} {} ", s.key(), s.title());
        spans.push(if *s == app.screen {
            Span::styled(
                label,
                Style::new()
                    .fg(t.on_accent)
                    .bg(t.lead)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(label, Style::new().fg(t.muted))
        });
        spans.push(Span::raw(" "));
    }
    let used = Line::from(spans.clone()).width();
    frame.render_widget(Line::from(spans), area);
    if app.screen == Screen::Dashboard && app.dashboard.classic {
        let look = &app.look;
        let right = Line::from(vec![
            Span::styled(
                format!("{} ", look.icons.get(look::Icon::Theme)),
                Style::new().fg(t.label),
            ),
            Span::styled(
                format!("{} · {} layout ", t.name, look.layout_spec().name),
                Style::new().fg(t.dim),
            ),
        ])
        .right_aligned();
        if area.width as usize > right.width() + used + 2 {
            frame.render_widget(right, area);
        }
    }
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App, t: &Theme) {
    let line = match &app.mode {
        Mode::Input(purpose, input) => {
            let prompt = match purpose {
                InputPurpose::NewSession => "session name".to_string(),
                InputPurpose::WorkspaceSession => format!(
                    "workspace ({})",
                    app.workspaces
                        .iter()
                        .map(|w| w.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
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
                InputPurpose::PlanNote => "what should the plan change?".to_string(),
                InputPurpose::BoardAnswer => format!(
                    "answer · {}",
                    app.answering_question().unwrap_or_else(|| "—".into())
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
                Span::styled(format!(" {prompt}: "), Style::new().fg(t.lead)),
                Span::raw(before),
                Span::styled(at, Style::new().add_modifier(Modifier::REVERSED)),
                Span::raw(after),
                Span::styled(keys, Style::new().fg(t.muted)),
            ])
        }
        Mode::Confirm(_) => Line::from(Span::styled(
            " irreversible: name the target to approve · esc keeps the call held",
            Style::new().fg(t.error),
        )),
        Mode::Shelve => Line::from(vec![
            Span::styled(" shelve: ", Style::new().fg(t.lead)),
            Span::styled("1", Style::new().add_modifier(Modifier::BOLD)),
            Span::styled(" 15 min · ", Style::new().fg(t.muted)),
            Span::styled("2", Style::new().add_modifier(Modifier::BOLD)),
            Span::styled(" 60 min · ", Style::new().fg(t.muted)),
            Span::styled("3", Style::new().add_modifier(Modifier::BOLD)),
            Span::styled(
                " until its job ends · esc cancel (it comes back by itself)",
                Style::new().fg(t.muted),
            ),
        ]),
        Mode::Normal => match (&app.error, app.prompts.selected_prompt(), app.toast()) {
            (Some(e), _, _) => {
                Line::from(Span::styled(format!(" ! {e}"), Style::new().fg(t.error)))
            }
            (None, _, _) if app.notice.is_some() => Line::from(Span::styled(
                format!(" {}", app.notice.as_deref().unwrap_or_default()),
                Style::new().fg(t.ok),
            )),
            (None, Some(p), _) if app.prompts.confirm_delete => Line::from(Span::styled(
                format!(
                    " press x again to delete \"{}\" · any other key keeps it",
                    p.name
                ),
                Style::new().fg(t.warn),
            )),
            (None, _, Some(toast)) => Line::from(vec![
                Span::styled(
                    format!(" {} ", app.look.icons.get(look::Icon::Theme)),
                    Style::new()
                        .fg(t.on_accent)
                        .bg(t.border_focus)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" {toast}"), Style::new().fg(t.border_focus)),
            ]),
            _ if app.screen == Screen::Dashboard && app.dashboard.classic => {
                dashboard::status::line(app, &dashboard::View::of(app), t)
            }
            _ => Line::from(Span::styled(hint_line(app), Style::new().fg(t.muted))),
        },
    };
    frame.render_widget(line, area);
}

/// The hints, with `q` letting the window go when the background service
/// runs the app.
fn hint_line(app: &App) -> String {
    let hints = hints(app);
    if app.in_service() {
        hints.replace("q quit", "q detach")
    } else {
        hints.to_string()
    }
}

pub(crate) fn hints(app: &App) -> &'static str {
    if app.prompts.form.is_some() {
        return " tab next field · ctrl+s save · esc cancel";
    }
    if app.screen == Screen::Missions && app.missions_screen.composer.is_some() {
        return " tab next field · ctrl+s save & lint · ctrl+f protect oracles · ctrl+r save & run · esc close";
    }
    match app.screen {
        Screen::Mission => {
            " ↑↓ select · ⏎ open · y/n/a answer · A batch · s shelve · F focus · tab wall · p prompt · q quit"
        }
        Screen::Missions => {
            " n new · e edit · l lint · f protect · p plan first · r run · y/n/c plan · ⏎ focus · q quit"
        }
        Screen::Dashboard if !app.dashboard.classic => {
            " ↑↓ scroll · d diff · t open session · p prompt · v review · D classic · q quit"
        }
        Screen::Dashboard => {
            " p prompt · P route · v review · ↑↓ session · t theme · L layout · tab focus · q quit"
        }
        Screen::Queue => " c cancel · r retry · +/- priority · enter view · ↑↓ select · q quit",
        Screen::Ledger => {
            " / search · o filter · v verify · l labels · r re-run · f fork · enter view · ↑↓ select · q quit"
        }
        Screen::Prompts => {
            " n new · e edit · x delete · d dispatch · D route · ↑↓ select · 0-7 screens · q quit"
        }
        Screen::Sessions => {
            " n new session · w on a workspace · l lean on/off · ↑↓ select · 0-7 screens · q quit"
        }
        Screen::Board => " ↑↓ select · r answer a question · x close it · 0-7 screens · q quit",
    }
}

pub(crate) fn render_to_string(app: &App, w: u16, h: u16) -> String {
    buffer_to_string(&render_buffer(app, w, h))
}

/// The buffer as text, one line per row.
pub(crate) fn buffer_to_string(buf: &ratatui::buffer::Buffer) -> String {
    let area = buf.area;
    (area.y..area.bottom())
        .map(|y| {
            (area.x..area.right())
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Renders the app into a buffer, for colour assertions.
pub(crate) fn render_buffer(app: &App, w: u16, h: u16) -> ratatui::buffer::Buffer {
    use ratatui::{Terminal, backend::TestBackend};
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| draw(f, app)).unwrap();
    terminal.backend().buffer().clone()
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
        assert!(out.contains("4 queue"));
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
