//! Rendering. Reads `App` and does no I/O; the only state it touches is the
//! editor's scroll offset.

pub mod dashboard;
pub mod editor;
pub mod input;
pub mod prompts;
pub mod sessions;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};

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
        Screen::Sessions => sessions::render(frame, body, app),
        other => placeholder(frame, body, other),
    }

    draw_status(frame, status, app);
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

fn placeholder(frame: &mut Frame, area: Rect, screen: Screen) {
    let block = Block::bordered()
        .border_style(Style::new().fg(MUTED))
        .title(format!(" {} ", screen.title()));
    frame.render_widget(
        Paragraph::new("coming in a later milestone")
            .fg(MUTED)
            .block(block),
        area,
    );
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
            };
            let (before, at, after) = input.split_at_cursor();
            Line::from(vec![
                Span::styled(format!(" {prompt}: "), Style::new().fg(ORANGE)),
                Span::raw(before),
                Span::styled(at, Style::new().add_modifier(Modifier::REVERSED)),
                Span::raw(after),
                Span::styled("   enter send · esc cancel", Style::new().fg(MUTED)),
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
        Screen::Dashboard => " p prompt · ↑↓ session · 1-5 screens · q quit",
        Screen::Prompts => {
            " n new · e edit · x delete · d dispatch · ↑↓ select · 1-5 screens · q quit"
        }
        Screen::Sessions => " n new session · ↑↓ select · 1-5 screens · q quit",
        _ => " 1-5 screens · q quit",
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
}
