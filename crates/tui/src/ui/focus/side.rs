//! The columns beside the answer: the reviewer (only when a review is
//! used), the lead's plan and the agents.

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_domain::telemetry::TodoStatus;
use swarmux_domain::{ReviewVerdict, SubagentStatus, display_name};

use super::parts::wrap;
use crate::app::App;
use crate::focus::stream::RowKind;
use crate::focus::{Focus, ReviewColumn};
use crate::ui::dashboard::{dim, fit, label};
use crate::ui::look::Icon;
use crate::ui::mission::{glyph, panel};

pub fn review(frame: &mut Frame, area: Rect, app: &App, r: &ReviewColumn) {
    let t = &app.look.theme;
    let (icon, color) = match r.verdict {
        Some(ReviewVerdict::Pass) => (Icon::Result, t.ok),
        Some(ReviewVerdict::Concerns) => (Icon::Concerns, t.warn),
        None if r.running => (Icon::StateWait, t.muted),
        None => (Icon::Failed, t.error),
    };
    let meta = Line::from(Span::styled(
        format!(" {} {} ", glyph(app, icon), fit(&r.headline, 18)),
        Style::new().fg(color),
    ));
    let title = format!("reviewer · {}", display_name(&r.model));
    let block = panel(t, glyph(app, Icon::Reviewer), &title, meta, t.border);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 12 || inner.height == 0 {
        return;
    }
    let w = usize::from(inner.width);
    let mut lines: Vec<Line> = Vec::new();
    let mut facts = Vec::new();
    if let Some(n) = r.diffs_checked {
        facts.push(format!("{n} diffs checked"));
    }
    if let Some(c) = r.cost_usd {
        facts.push(format!("${c:.3}"));
    }
    if !facts.is_empty() {
        lines.push(Line::from(Span::styled(
            facts.join(" · "),
            Style::new().fg(t.muted),
        )));
    }
    if r.verdict.is_none() {
        lines.extend(wrap(&[(r.headline.clone(), Style::new().fg(t.text))], w));
    }
    for (i, concern) in r.concerns.iter().enumerate() {
        let head = format!("{}. ", i + 1);
        let pad = " ".repeat(head.len());
        let wrapped = wrap(
            &[(concern.clone(), Style::new().fg(t.text))],
            w.saturating_sub(head.len()),
        );
        for (j, l) in wrapped.into_iter().enumerate() {
            let lead = if j == 0 { head.clone() } else { pad.clone() };
            let mut spans = vec![Span::styled(lead, Style::new().fg(t.warn))];
            spans.extend(l.spans);
            lines.push(Line::from(spans));
        }
    }
    // What the reviewer itself did, newest last, in what is left.
    let room = usize::from(inner.height).saturating_sub(lines.len() + 1);
    let calls: Vec<Line> = r
        .rows
        .iter()
        .filter_map(|row| match &row.kind {
            RowKind::Call { tool, subject } => Some(Line::from(vec![
                Span::styled(
                    format!("{} ", glyph(app, Icon::Call)),
                    Style::new().fg(t.reviewer),
                ),
                Span::styled(
                    fit(&format!("{tool} {subject}"), w.saturating_sub(2)),
                    Style::new().fg(t.muted),
                ),
            ])),
            _ => None,
        })
        .collect();
    if room > 0 && !calls.is_empty() {
        lines.push(Line::from(label(t, "looked at")));
        let skip = calls.len().saturating_sub(room);
        lines.extend(calls.into_iter().skip(skip));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

pub fn plan(frame: &mut Frame, area: Rect, app: &App, f: &Focus) {
    let t = &app.look.theme;
    let done = f
        .todos
        .iter()
        .filter(|d| d.status == TodoStatus::Completed)
        .count();
    let meta = Line::from(dim(t, format!(" {done}/{} ", f.todos.len())));
    let block = panel(t, glyph(app, Icon::Todos), "plan", meta, t.border);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 12 || inner.height == 0 {
        return;
    }
    if f.todos.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(dim(t, "the agent keeps no todo list"))),
            Rect { height: 1, ..inner },
        );
        return;
    }
    let w = usize::from(inner.width).saturating_sub(2);
    let lines: Vec<Line> = f
        .todos
        .iter()
        .take(usize::from(inner.height))
        .map(|d| {
            let (icon, color) = match d.status {
                TodoStatus::Completed => (Icon::Result, t.ok),
                TodoStatus::InProgress => (Icon::StateRun, t.lead),
                _ => (Icon::StateIdle, t.muted),
            };
            Line::from(vec![
                Span::styled(format!("{} ", glyph(app, icon)), Style::new().fg(color)),
                Span::styled(fit(&d.content, w), Style::new().fg(t.text)),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

pub fn agents(frame: &mut Frame, area: Rect, app: &App, f: &Focus) {
    let t = &app.look.theme;
    let running = f
        .agents
        .iter()
        .filter(|a| a.status == SubagentStatus::Running)
        .count();
    let meta = Line::from(dim(t, format!(" {running}/{} active ", f.agents.len())));
    let block = panel(t, glyph(app, Icon::Subagents), "agents", meta, t.border);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 12 || inner.height == 0 {
        return;
    }
    if f.agents.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(dim(t, "no subagents"))),
            Rect { height: 1, ..inner },
        );
        return;
    }
    let lines: Vec<Line> = f
        .agents
        .iter()
        .take(usize::from(inner.height))
        .map(|a| {
            let (icon, color) = match a.status {
                SubagentStatus::Running => (Icon::StateRun, t.ok),
                SubagentStatus::Done => (Icon::StateDone, t.muted),
                _ => (Icon::StateErr, t.error),
            };
            let model = a
                .model
                .as_deref()
                .map_or_else(String::new, |m| format!(" {}", display_name(m)));
            Line::from(vec![
                Span::styled(format!("{} ", glyph(app, icon)), Style::new().fg(color)),
                Span::styled(
                    fit(&a.kind, 14),
                    Style::new().fg(t.model(a.model.as_deref())),
                ),
                Span::styled(model, Style::new().fg(t.muted)),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}
