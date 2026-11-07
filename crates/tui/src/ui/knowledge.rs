//! The Knowledge screen: the registry on top, then each group of items.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_domain::knowledge::{EnforcerKind, Item, Status, Trust};

use crate::app::App;
use crate::knowledge::KnowledgeRow;
use crate::theme::Theme;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let block = super::rounded(t)
        .border_style(Style::new().fg(t.muted))
        .title(" knowledge ");
    let Some(registry) = app.knowledge_registry() else {
        frame.render_widget(
            Paragraph::new("knowledge is not available in this window")
                .fg(t.muted)
                .block(block),
            area,
        );
        return;
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(
            registry.to_string(),
            Style::new().fg(t.label).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "   {} shadow · {} stale · {} proposed",
                registry.shadow, registry.stale, registry.proposed
            ),
            Style::new().fg(t.muted),
        ),
    ])];
    let rows = app.knowledge_rows();
    if rows.is_empty() {
        lines.push(Line::from(Span::styled(
            "no rules, facts or decisions yet: a adds one, `swarmux knowledge init <workspace>` drafts a workspace's map",
            Style::new().fg(t.muted),
        )));
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(ratatui::widgets::Wrap { trim: true })
                .block(block),
            area,
        );
        return;
    }
    let mut at = 0;
    let mut selected_line = 0;
    for row in &rows {
        match row {
            KnowledgeRow::Header(name, count) => lines.push(Line::from(vec![
                Span::styled(
                    format!("{name} "),
                    Style::new().fg(t.label).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("({count})"), Style::new().fg(t.muted)),
            ])),
            KnowledgeRow::Item(i) => {
                let chosen = at == app.knowledge_screen.selected;
                if chosen {
                    selected_line = lines.len();
                }
                lines.push(item_line(t, i, chosen));
                at += 1;
            }
        }
    }
    let inner = area.height.saturating_sub(2) as usize;
    let top = selected_line.saturating_sub(inner.saturating_sub(2));
    let shown: Vec<Line> = lines.into_iter().skip(top).collect();
    frame.render_widget(Paragraph::new(shown).block(block), area);
}

fn item_line<'a>(t: &Theme, i: &Item, chosen: bool) -> Line<'a> {
    let colour = match i.status {
        Status::Stale => t.warn,
        Status::Enforced if i.enforcer_kind() != Some(EnforcerKind::Prompt) => t.ok,
        Status::Shadow | Status::Proposed => t.muted,
        _ => t.text,
    };
    let enforcer = i
        .enforcer_kind()
        .map_or(String::new(), |e| format!(" {}", e.as_str()));
    let mut tags = String::new();
    if i.core {
        tags.push_str(" core");
    }
    if i.trust == Trust::Untrusted {
        tags.push_str(" untrusted");
    }
    if i.counters.shadow_hits > 0 {
        tags.push_str(&format!(" · would have acted {}×", i.counters.shadow_hits));
    }
    if i.counters.hits > 0 {
        tags.push_str(&format!(" · used {}×", i.counters.hits));
    }
    if i.last_check == Some(false) {
        tags.push_str(" · check failing");
    }
    let mark = if chosen { "▸ " } else { "  " };
    let mut style = Style::new().fg(colour);
    if chosen {
        style = style.add_modifier(Modifier::REVERSED);
    }
    Line::from(vec![
        Span::styled(format!("{mark}{:<5}", i.id), style),
        Span::styled(
            format!(" {:<9}{enforcer}", i.status.as_str()),
            Style::new().fg(t.muted),
        ),
        Span::raw("  "),
        Span::styled(
            swarmux_domain::board::clean(&i.text, 160),
            Style::new().fg(colour),
        ),
        Span::styled(tags, Style::new().fg(t.muted)),
    ])
}
