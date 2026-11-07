//! The Follow-ups screen: the brief's headline, decision cards, approved
//! work waiting to become a mission, and the standing orders.
//!
//! Card text came from an agent, so it is cleaned (`BoardEntry`'s rules)
//! when stored and shown on one quoted line.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use swarmux_domain::followup::{Card, FollowUp};

use crate::app::App;
use crate::theme::Theme;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let block = super::rounded(t)
        .border_style(Style::new().fg(t.muted))
        .title(" follow-ups ");
    if !app.followups_on() {
        frame.render_widget(
            Paragraph::new(
                "follow-through is off: set enabled = true under [followups] in config.toml",
            )
            .fg(t.muted)
            .block(block),
            area,
        );
        return;
    }
    let s = &app.followup_screen;
    let mut lines: Vec<Line> = Vec::new();
    if let Some(b) = &s.brief {
        lines.push(Line::from(Span::styled(
            b.headline(),
            Style::new().fg(t.label).add_modifier(Modifier::BOLD),
        )));
        if let Some(note) = b.spot_note() {
            lines.push(Line::from(Span::styled(note, Style::new().fg(t.warn))));
        }
        lines.push(Line::default());
    }
    let mut selected_line = 0;
    if s.cards.is_empty() {
        lines.push(Line::from(Span::styled(
            "nothing is waiting for you",
            Style::new().fg(t.muted),
        )));
    }
    for (i, f) in s.cards.iter().enumerate() {
        let chosen = i == s.selected;
        if chosen {
            selected_line = lines.len();
        }
        card_lines(t, f, chosen, &mut lines);
    }
    if !s.intents.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            format!("approved, not yet a mission ({})", s.intents.len()),
            Style::new().fg(t.label).add_modifier(Modifier::BOLD),
        )));
        for (i, intent) in s.intents.iter().enumerate() {
            let chosen = s.cards.len() + i == s.selected;
            if chosen {
                selected_line = lines.len();
            }
            let marker = if chosen { "› " } else { "  " };
            let mut style = Style::new().fg(t.ok);
            if chosen {
                style = style.add_modifier(Modifier::REVERSED);
            }
            lines.push(Line::from(Span::styled(
                format!(
                    "{marker}#{} \"{}\"  {}",
                    intent.followup,
                    intent.text,
                    if intent.repo.is_empty() {
                        "—"
                    } else {
                        &intent.repo
                    }
                ),
                style,
            )));
        }
    }
    let orders = app.standing_orders();
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        format!("standing orders ({})", orders.len()),
        Style::new().fg(t.label).add_modifier(Modifier::BOLD),
    )));
    if orders.is_empty() {
        lines.push(Line::from(Span::styled(
            "  none: add [[followups.order]] to config.toml (see guide/follow-through.md)",
            Style::new().fg(t.muted),
        )));
    }
    for o in &orders {
        let kinds = if o.kinds.is_empty() {
            "any".to_string()
        } else {
            o.kinds
                .iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join("/")
        };
        let mut rest = Vec::new();
        if !o.repos.is_empty() {
            rest.push(format!("repo {}", o.repos.join(",")));
        }
        if !o.paths.is_empty() {
            rest.push(format!("paths {}", o.paths.join(",")));
        }
        if !o.keywords.is_empty() {
            rest.push(format!("words {}", o.keywords.join(",")));
        }
        if let Some(c) = o.max_cost_usd {
            rest.push(format!("≤ ${c:.2}"));
        }
        lines.push(Line::from(Span::styled(
            format!(
                "  {}: {} {} {}",
                o.id,
                o.action.as_str(),
                kinds,
                rest.join(" · ")
            ),
            Style::new().fg(t.text),
        )));
    }
    let inner = area.height.saturating_sub(2) as usize;
    let top = selected_line.saturating_sub(inner.saturating_sub(6));
    let shown: Vec<Line> = lines.into_iter().skip(top).collect();
    frame.render_widget(
        Paragraph::new(shown)
            .wrap(Wrap { trim: false })
            .block(block),
        area,
    );
}

fn card_lines<'a>(t: &Theme, f: &FollowUp, chosen: bool, out: &mut Vec<Line<'a>>) {
    let c = Card::of(f, false);
    let marker = if chosen { "› " } else { "  " };
    let mut head = Style::new().fg(match f.kind.as_str() {
        "risk" | "blocker" => t.warn,
        "decision" | "question" => t.label,
        _ => t.text,
    });
    if chosen {
        head = head.add_modifier(Modifier::REVERSED | Modifier::BOLD);
    }
    out.push(Line::from(Span::styled(
        format!(
            "{marker}#{} [{}] \"{}\"{}",
            c.id,
            c.kind.as_str(),
            c.what,
            if c.repo.is_empty() {
                String::new()
            } else {
                format!("  ({})", c.repo)
            }
        ),
        head,
    )));
    let dim = Style::new().fg(t.muted);
    out.push(Line::from(Span::styled(
        format!("      why: {}", c.why),
        dim,
    )));
    out.push(Line::from(Span::styled(
        format!("      from: {}", c.evidence),
        dim,
    )));
    out.push(Line::from(Span::styled(
        format!("      you might say no because: {}", c.against),
        dim,
    )));
}
