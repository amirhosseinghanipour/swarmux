//! The Board screen: entries grouped by kind, live claims and open
//! questions first.
//!
//! Every entry is drawn through `BoardEntry::line`, so text an agent wrote
//! appears cleaned, quoted and tagged with its job, task and time.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_domain::board::{BoardEntry, BoardKind};

use crate::app::App;
use crate::board::BoardRow;
use crate::theme::Theme;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let block = super::rounded(t)
        .border_style(Style::new().fg(t.muted))
        .title(" board ");
    if !app.board_on() {
        frame.render_widget(
            Paragraph::new("the board is off: set enabled = true under [board] in config.toml")
                .fg(t.muted)
                .block(block),
            area,
        );
        return;
    }
    let rows = app.board_rows();
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(
                "nothing on the board yet. Agents of a mission write here with the \
                 swarmux-board tools: claims, facts, decisions, questions.",
            )
            .fg(t.muted)
            .wrap(ratatui::widgets::Wrap { trim: true })
            .block(block),
            area,
        );
        return;
    }
    let mut lines = Vec::new();
    let mut at = 0;
    let mut selected_line = 0;
    for row in &rows {
        match row {
            BoardRow::Header(name, count) => lines.push(Line::from(vec![
                Span::styled(
                    format!("{name} "),
                    Style::new().fg(t.label).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("({count})"), Style::new().fg(t.muted)),
            ])),
            BoardRow::Entry(e) => {
                let chosen = at == app.board_screen.selected;
                if chosen {
                    selected_line = lines.len();
                }
                lines.push(entry_line(t, e, chosen));
                at += 1;
            }
        }
    }
    let inner = area.height.saturating_sub(2) as usize;
    let top = selected_line.saturating_sub(inner.saturating_sub(2));
    let shown: Vec<Line> = lines.into_iter().skip(top).collect();
    frame.render_widget(Paragraph::new(shown).block(block), area);
}

fn entry_line<'a>(t: &Theme, e: &BoardEntry, chosen: bool) -> Line<'a> {
    let colour = match e.kind {
        BoardKind::Question if e.is_open_question() => t.warn,
        BoardKind::Claim if e.is_live_claim() => t.lead,
        BoardKind::Decision => t.ok,
        BoardKind::Fact => t.text,
        BoardKind::Followup => t.label,
        _ => t.muted,
    };
    let mut style = Style::new().fg(colour);
    if e.is_live_claim() || e.is_open_question() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if chosen {
        style = style.add_modifier(Modifier::REVERSED);
    }
    let marker = if chosen { "› " } else { "  " };
    Line::from(Span::styled(format!("{marker}{}", e.line()), style))
}
