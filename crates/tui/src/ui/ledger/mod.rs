//! The Ledger screen: what happened, what it cost and whether it can be
//! proven. A strip of numbers and the digest of what happened while the
//! user was away on top; run cards on the left; the selected run, its
//! bundle and its verification on the right.

mod detail;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};
use swarmux_domain::evidence::{LabelKind, RunCard, RunOutcome};
use swarmux_domain::{display_name, iso_minute};

use crate::app::App;
use crate::theme::Theme;
use crate::ui::look::IconSet;

/// Rows of the numbers strip, borders included.
const HEADER_ROWS: u16 = 5;
/// Windows shorter than this skip the strip.
const HEADER_MIN_HEIGHT: u16 = 16;
/// Lines a card takes, and the gap after it.
const CARD_LINES: usize = 2;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let body = if area.height >= HEADER_MIN_HEIGHT {
        let [head, body] =
            Layout::vertical([Constraint::Length(HEADER_ROWS), Constraint::Fill(1)]).areas(area);
        render_header(frame, head, app);
        body
    } else {
        area
    };
    let [cards, side] =
        Layout::horizontal([Constraint::Percentage(56), Constraint::Percentage(44)]).areas(body);
    render_cards(frame, cards, app);
    detail::render(frame, side, app);
}

pub(super) fn panel(t: &Theme, title: String) -> Block<'static> {
    super::rounded(t)
        .border_style(Style::new().fg(t.muted))
        .title(title)
}

/// A glyph from the Unicode set, or its ASCII stand-in.
pub(super) fn g(app: &App, unicode: &'static str, ascii: &'static str) -> &'static str {
    if app.look.icons == IconSet::Ascii {
        ascii
    } else {
        unicode
    }
}

/// How an outcome looks: its mark and colour.
fn outcome_look(app: &App, outcome: RunOutcome) -> (&'static str, Color) {
    let t = &app.look.theme;
    match outcome {
        RunOutcome::Delivered => (g(app, "✓", "+"), t.ok),
        RunOutcome::Escalated => (g(app, "⚑", "!"), t.error),
        RunOutcome::Refused => (g(app, "⊘", "x"), t.error),
        RunOutcome::Failed => (g(app, "✗", "x"), t.error),
        RunOutcome::Cancelled => (g(app, "⊘", "x"), t.muted),
        RunOutcome::Queued => (g(app, "…", "~"), t.muted),
        RunOutcome::Running => (g(app, "▶", "*"), t.lead),
        RunOutcome::Done => (g(app, "·", "."), t.ok),
    }
}

/// The colour of an outcome label.
pub(super) fn label_color(t: &Theme, kind: LabelKind) -> Color {
    match kind {
        LabelKind::Merged => t.ok,
        LabelKind::Reverted => t.error,
        LabelKind::PrOpen => t.lead,
        LabelKind::Closed => t.muted,
    }
}

/// `label #9`: the newest label of a card.
fn label_text(card: &RunCard) -> Option<(String, LabelKind)> {
    card.labels.last().map(|l| {
        (
            format!("{} #{}", l.kind.as_str().replace('_', " "), l.pr),
            l.kind,
        )
    })
}

/// `sonnet+haiku`: the models by the dollars they were paid.
pub(super) fn model_mix(models: &[(String, f64)]) -> String {
    let names: Vec<String> = models
        .iter()
        .map(|(m, _)| {
            display_name(m)
                .split(' ')
                .next()
                .unwrap_or("")
                .to_lowercase()
        })
        .fold(Vec::new(), |mut acc, n| {
            if !n.is_empty() && !acc.contains(&n) {
                acc.push(n);
            }
            acc
        });
    if names.is_empty() {
        "—".to_string()
    } else {
        names.join("+")
    }
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let l = &app.ledger;
    let s = &l.stats;
    let muted = Style::new().fg(t.muted);
    let sep = || Span::styled(" · ", Style::new().fg(t.dim));
    let figure = |text: String, color: Color| Span::styled(text, Style::new().fg(color));
    let mut numbers: Vec<Span> = Vec::new();
    match (s.pass_k, s.pass_rate) {
        (Some(k), Some(rate)) => {
            let color = if rate >= 0.7 { t.ok } else { t.warn };
            numbers.push(figure(format!("pass^3 {k:.2}"), color));
            numbers.push(sep());
            numbers.push(figure(
                format!("{}/{} delivered", s.delivered, s.missions),
                t.text,
            ));
        }
        _ => numbers.push(Span::styled("no mission has ended yet", muted)),
    }
    if let Some(c) = s.cost_per_verified {
        numbers.push(sep());
        numbers.push(figure(format!("${c:.2}/verified outcome"), t.text));
    }
    if let Some(i) = s.interventions_per_task {
        numbers.push(sep());
        numbers.push(figure(format!("{i:.1} interventions/task"), t.text));
    }
    if s.merged + s.reverted > 0 {
        numbers.push(sep());
        numbers.push(figure(format!("{} merged", s.merged), t.ok));
        numbers.push(Span::styled(" · ", Style::new().fg(t.dim)));
        numbers.push(figure(format!("{} reverted", s.reverted), t.error));
    }

    let scope = if l.away {
        "while you were away"
    } else {
        "last 24 h"
    };
    let mut digest = vec![
        Span::styled(format!("{}d ", l.heat.len()), muted),
        Span::styled(heat(app, &l.heat), Style::new().fg(t.lead)),
        Span::raw("  "),
        Span::styled(format!("{scope}: "), Style::new().fg(t.label)),
        Span::styled(l.brief.headline(), Style::new().fg(t.text)),
    ];
    if l.brief.is_empty() && l.away {
        digest.truncate(5);
    }
    let attention = if l.brief.needs_you.is_empty() {
        Line::styled("nothing needs you", muted)
    } else {
        Line::from(vec![
            Span::styled("needs you: ", Style::new().fg(t.error)),
            Span::styled(l.brief.needs_you.join("  ·  "), Style::new().fg(t.text)),
        ])
    };
    frame.render_widget(
        Paragraph::new(vec![Line::from(numbers), Line::from(digest), attention])
            .block(panel(t, " the record ".into())),
        area,
    );
}

/// One block per day, taller for busier days.
fn heat(app: &App, counts: &[u32]) -> String {
    let ramp: &[char] = if app.look.icons == IconSet::Ascii {
        &[' ', '.', ':', '-', '=', '+', '*', '#', '%']
    } else {
        &[' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█']
    };
    let top = counts.iter().copied().max().unwrap_or(0).max(1);
    counts
        .iter()
        .map(|&n| {
            let level = if n == 0 {
                0
            } else {
                (n as usize * (ramp.len() - 2)).div_ceil(top as usize) + 1
            };
            ramp[level.min(ramp.len() - 1)]
        })
        .collect()
}

fn render_cards(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let l = &app.ledger;
    let mut title = format!(" ledger (UTC) · {}", l.filter.title());
    if !l.query.is_empty() {
        title.push_str(&format!(" · \"{}\" · {} found", l.query, l.cards.len()));
    }
    title.push(' ');
    if l.cards.is_empty() {
        let hint = if !l.query.is_empty() || l.filter != crate::ledger::Filter::All {
            "nothing matches: press / to change the search, o for the filter"
        } else {
            "no runs yet"
        };
        frame.render_widget(
            Paragraph::new(hint).fg(t.muted).block(panel(t, title)),
            area,
        );
        return;
    }
    let block = panel(t, title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let per = CARD_LINES + 1;
    let visible = (inner.height as usize).div_ceil(per).max(1);
    let first = (l.selected + 1).saturating_sub(visible);
    let mut lines: Vec<Line> = Vec::new();
    for (i, card) in l.cards.iter().enumerate().skip(first).take(visible) {
        lines.extend(card_lines(app, card, i == l.selected, inner.width as usize));
        lines.push(Line::raw(""));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The two lines of a run card: outcome, title and cost; then the
/// evidence, models, labels and time.
fn card_lines(app: &App, card: &RunCard, selected: bool, w: usize) -> [Line<'static>; 2] {
    let t = &app.look.theme;
    let (mark, color) = outcome_look(app, card.outcome);
    let cursor = if selected { g(app, "› ", "> ") } else { "  " };
    let word = format!("{:<10}", card.outcome.as_str());
    let cost = format!("${:.2}", card.cost_usd);
    let used = cursor.chars().count() + 2 + word.chars().count() + cost.chars().count() + 1;
    let room = w.saturating_sub(used);
    let title = fit(&card.title, room);
    let pad = " ".repeat(room.saturating_sub(title.chars().count()) + 1);
    let weight = if selected {
        Style::new().add_modifier(ratatui::style::Modifier::BOLD)
    } else {
        Style::new()
    };
    let first = Line::from(vec![
        Span::styled(cursor, Style::new().fg(t.lead)),
        Span::styled(format!("{mark} "), Style::new().fg(color)),
        Span::styled(word, Style::new().fg(color)),
        Span::styled(title, weight.fg(t.text)),
        Span::raw(pad),
        Span::styled(cost, Style::new().fg(t.muted)),
    ]);
    let muted = Style::new().fg(t.muted);
    let dot = || Span::styled(" · ", Style::new().fg(t.dim));
    let mut second = vec![
        Span::raw("    "),
        Span::styled(iso_minute(card.at)[5..].to_string(), muted),
        dot(),
    ];
    match &card.evidence {
        Some(e) => {
            let all = e.checks_passed == e.checks_total;
            second.push(Span::styled(
                format!(
                    "{} {}/{} {}",
                    if all {
                        g(app, "✓", "+")
                    } else {
                        g(app, "✗", "x")
                    },
                    e.checks_passed,
                    e.checks_total,
                    e.digest
                ),
                Style::new().fg(if all { t.ok } else { t.error }),
            ));
        }
        None => second.push(Span::styled("no bundle", Style::new().fg(t.dim))),
    }
    second.push(dot());
    second.push(Span::styled(model_mix(&card.models), muted));
    if let Some((text, kind)) = label_text(card) {
        second.push(dot());
        second.push(Span::styled(text, Style::new().fg(label_color(t, kind))));
    }
    // Last: on a narrow card the label matters more than the cache share.
    second.push(dot());
    second.push(Span::styled(
        card.cache.map_or_else(
            || "cache —".to_string(),
            |c| format!("cache {:.0}%", c * 100.0),
        ),
        muted,
    ));
    [first, Line::from(second)]
}

/// `text` cut to `max` characters, with an ellipsis when it was cut.
pub(super) fn fit(text: &str, max: usize) -> String {
    let first = text.lines().next().unwrap_or_default();
    if first.chars().count() <= max {
        return first.to_string();
    }
    let keep = max.saturating_sub(1);
    let mut out: String = first.chars().take(keep).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests;
