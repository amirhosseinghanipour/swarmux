//! The `?` overlay: the keys that work here, from the live keymap.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
};

use super::rounded;
use crate::app::App;
use crate::help::{HelpLine, HelpState, lines};
use crate::keys::HELP_TOGGLE;
use crate::theme::Theme;

/// Widest the panel gets.
pub(crate) const MAX_WIDTH: u16 = 100;
/// Most the key column takes.
const MAX_KEYS: usize = 22;

pub fn render(frame: &mut Frame, area: Rect, app: &App, state: &HelpState) {
    let t = &app.look.theme;
    let content = lines(app);
    let keys_w = content
        .iter()
        .filter_map(|l| match l {
            HelpLine::Row { keys, .. } => Some(Line::from(keys.as_str()).width()),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        .min(MAX_KEYS);
    let id_w = content
        .iter()
        .filter_map(|l| match l {
            HelpLine::Row { id, .. } => Some(id.len()),
            _ => None,
        })
        .max()
        .unwrap_or(0);

    let width = area.width.min(MAX_WIDTH);
    let height = area.height.min(content.len() as u16 + 2);
    if width < 12 || height < 3 {
        return;
    }
    let panel = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let inner_rows = usize::from(height - 2);
    let scroll = state
        .scroll
        .get()
        .min(content.len().saturating_sub(inner_rows));
    state.scroll.set(scroll);

    let shown: Vec<Line> = content
        .iter()
        .skip(scroll)
        .take(inner_rows)
        .map(|l| line(l, t, keys_w, id_w))
        .collect();

    let close = app.key_of(HELP_TOGGLE).map_or_else(
        || "esc closes".to_string(),
        |k| format!("{k} or esc closes"),
    );
    let more = if content.len() > inner_rows {
        " · scroll with the list keys"
    } else {
        ""
    };
    let block = rounded(t)
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!("keys · {}", app.screen.title()),
                Style::new().fg(t.title).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ]))
        .title_bottom(Line::from(Span::styled(
            format!(" {close}{more} "),
            Style::new().fg(t.muted),
        )));
    // The key list is modal: nothing of the screen behind it shows, not even
    // beside the panel. Clear, then paint the theme's background again.
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::new().style(Style::new().bg(t.bg_paint()).fg(t.text)),
        area,
    );
    frame.render_widget(Paragraph::new(shown).block(block), panel);
}

fn line(l: &HelpLine, t: &Theme, keys_w: usize, id_w: usize) -> Line<'static> {
    match l {
        HelpLine::Heading { title, table } => Line::from(vec![
            Span::styled(
                format!(" {title}"),
                Style::new().fg(t.lead).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {table}"), Style::new().fg(t.dim)),
        ]),
        HelpLine::Row {
            keys,
            text,
            id,
            fixed,
        } => {
            let pad = keys_w.saturating_sub(Line::from(keys.as_str()).width());
            let mut spans = vec![
                Span::styled(
                    format!("   {keys}{}  ", " ".repeat(pad)),
                    Style::new().fg(t.text).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{id:<id_w$}  "), Style::new().fg(t.dim)),
                Span::styled(text.clone(), Style::new().fg(t.muted)),
            ];
            if *fixed {
                spans.push(Span::styled(" (fixed)", Style::new().fg(t.warn)));
            }
            Line::from(spans)
        }
        HelpLine::Note(text) => {
            Line::from(Span::styled(format!(" {text}"), Style::new().fg(t.warn)))
        }
        HelpLine::Gap => Line::default(),
    }
}
