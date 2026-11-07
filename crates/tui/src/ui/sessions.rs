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
        let text = if app.workspaces.is_empty() {
            "no sessions yet: press n to create one in the current folder"
        } else {
            "no sessions yet: press n to create one in the current folder, w for one on a workspace"
        };
        frame.render_widget(Paragraph::new(text).fg(t.muted).block(block), area);
        return;
    }
    // A workspace's sessions sit together under a line naming it and its
    // repositories; the list keeps them together.
    let mut items: Vec<ListItem> = Vec::new();
    let mut selected = 0;
    let mut group = None;
    for (i, s) in app.sessions.iter().enumerate() {
        let ws = app.workspace_of(s);
        let id = ws.map(|w| w.id);
        if id != group {
            group = id;
            if let Some(w) = ws {
                items.push(ListItem::new(Line::from(vec![
                    Span::styled(format!("◆ {} ", w.name), Style::new().fg(t.title).bold()),
                    Span::styled(
                        format!("{} ", crate::workspaces::repos(w)),
                        Style::new().fg(t.label),
                    ),
                    Span::styled(w.root.display().to_string(), Style::new().fg(t.muted)),
                ])));
            }
        }
        if i == app.selected {
            selected = items.len();
        }
        let indent = if ws.is_some() { "  " } else { "" };
        items.push(ListItem::new(Line::from(vec![
            Span::raw(format!("{indent}{:<20} ", s.name)),
            Span::styled(format!("{:<8} ", s.model), Style::new().fg(t.lead)),
            Span::styled(
                if s.lean { "lean " } else { "     " },
                Style::new().fg(t.ok),
            ),
            Span::styled(s.cwd.display().to_string(), Style::new().fg(t.muted)),
        ])));
    }
    let list = List::new(items)
        .block(block)
        .highlight_symbol("› ")
        .highlight_style(Style::new().bold());
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}
