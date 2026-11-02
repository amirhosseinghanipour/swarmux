use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph},
};

use crate::app::{App, Screen};

const ORANGE: Color = Color::Rgb(232, 140, 70);
const MUTED: Color = Color::Rgb(120, 120, 140);

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

    let tab_spans: Vec<Span> = Screen::ALL
        .iter()
        .enumerate()
        .flat_map(|(i, s)| {
            let label = format!(" {} {} ", i + 1, s.title());
            let span = if *s == app.screen {
                Span::styled(label, Style::new().fg(Color::Black).bg(ORANGE))
            } else {
                Span::styled(label, Style::new().fg(MUTED))
            };
            [span, Span::raw(" ")]
        })
        .collect();
    frame.render_widget(Line::from(tab_spans), tabs);

    let block = Block::bordered()
        .border_type(BorderType::Plain)
        .border_style(Style::new().fg(MUTED))
        .title(format!(" {} ", app.screen.title()));
    frame.render_widget(
        Paragraph::new("coming soon")
            .fg(MUTED)
            .block(block),
        body,
    );

    frame.render_widget(
        Line::from(" 1-5 screens · q quit ").style(Style::new().fg(MUTED)),
        status,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn renders_header_and_active_tab() {
        let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
        let app = App {
            screen: Screen::Queue,
            ..App::default()
        };
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("SWARMUX"));
        assert!(text.contains("3 queue"));
    }
}
