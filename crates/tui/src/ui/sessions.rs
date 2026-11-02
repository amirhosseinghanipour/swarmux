use ratatui::{
    Frame,
    layout::Rect,
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState, Paragraph},
};

use super::{GREEN, MUTED, ORANGE};
use crate::app::App;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered()
        .border_style(Style::new().fg(MUTED))
        .title(" sessions ");
    if app.sessions.is_empty() {
        frame.render_widget(
            Paragraph::new("no sessions yet: press n to create one in the current folder")
                .fg(MUTED)
                .block(block),
            area,
        );
        return;
    }
    let items: Vec<ListItem> = app
        .sessions
        .iter()
        .map(|s| {
            ListItem::new(Line::from(vec![
                Span::raw(format!("{:<20} ", s.name)),
                Span::styled(format!("{:<8} ", s.model), Style::new().fg(ORANGE)),
                Span::styled(
                    if s.lean { "lean " } else { "     " },
                    Style::new().fg(GREEN),
                ),
                Span::styled(s.cwd.display().to_string(), Style::new().fg(MUTED)),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(block)
        .highlight_symbol("› ")
        .highlight_style(Style::new().bold());
    let mut state = ListState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(list, area, &mut state);
}
