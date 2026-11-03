use ratatui::{
    Frame,
    layout::Rect,
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{List, ListItem, ListState, Paragraph},
};

use crate::app::App;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let block = super::rounded(t)
        .border_style(Style::new().fg(t.muted))
        .title(" sessions ");
    if app.sessions.is_empty() {
        frame.render_widget(
            Paragraph::new("no sessions yet: press n to create one in the current folder")
                .fg(t.muted)
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
                Span::styled(format!("{:<8} ", s.model), Style::new().fg(t.lead)),
                Span::styled(
                    if s.lean { "lean " } else { "     " },
                    Style::new().fg(t.ok),
                ),
                Span::styled(s.cwd.display().to_string(), Style::new().fg(t.muted)),
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
