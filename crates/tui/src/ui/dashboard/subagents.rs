//! Subagents: the delegate line, one card each (or a list when space is
//! short), and the hand-back to the lead with the review's verdict.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph},
};
use swarmux_domain::{
    JobProgress, ReviewState, ReviewVerdict, Subagent, SubagentStatus, display_name,
};

use super::View;
use super::widgets::{dim, fit, label, model_color, panel, sparkline, spinner, tokens};
use crate::ui::{DIM, GREEN, MUTED, ORANGE, RED};

/// Narrowest card; more subagents than fit are summed up in the last one.
const CARD_MIN_W: u16 = 24;

/// `: delegate · 3 subagents · parallel · effort — :` and a ▼ per card.
pub fn delegate(frame: &mut Frame, area: Rect, view: &View) {
    let [line, arrows] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
    let Some(job) = view.job.filter(|j| !j.subagents.is_empty()) else {
        frame.render_widget(
            Line::from(dim(": no subagents yet · the lead works alone :")).centered(),
            line,
        );
        return;
    };
    let running = job.subagents_running();
    let mode = if running >= 2 {
        "parallel"
    } else if running == 1 {
        "single"
    } else {
        "merged"
    };
    frame.render_widget(
        Line::from(dim(format!(
            ": delegate · {} subagents · {mode} · effort — :",
            job.subagents.len()
        )))
        .centered(),
        line,
    );
    let cols = card_areas(arrows, job.subagents.len());
    for col in cols {
        let x = col.x + col.width / 2;
        frame.render_widget(
            Span::styled("▼", Style::new().fg(DIM)),
            Rect::new(x, arrows.y, 1, 1),
        );
    }
}

/// Where each card goes: as many as fit, at least [`CARD_MIN_W`] wide.
fn card_areas(area: Rect, n: usize) -> Vec<Rect> {
    let fit = (area.width / CARD_MIN_W).max(1) as usize;
    let shown = n.min(fit);
    if shown == 0 {
        return Vec::new();
    }
    Layout::horizontal(vec![Constraint::Fill(1); shown])
        .spacing(1)
        .split(area)
        .to_vec()
}

/// One card per subagent.
pub fn cards(frame: &mut Frame, area: Rect, view: &View) {
    let Some(job) = view.job.filter(|j| !j.subagents.is_empty()) else {
        let block = Block::bordered()
            .border_type(BorderType::LightDoubleDashed)
            .border_style(Style::new().fg(DIM));
        frame.render_widget(
            Paragraph::new(vec![
                Line::raw(""),
                Line::from(dim("subagents appear here when the lead delegates")),
                Line::from(dim("(Task / Agent tool) · each gets an isolated context")),
            ])
            .centered()
            .block(block),
            area,
        );
        return;
    };
    let areas = card_areas(area, job.subagents.len());
    let hidden = job.subagents.len() - areas.len();
    for (i, (s, rect)) in job.subagents.iter().zip(&areas).enumerate() {
        let more = (i + 1 == areas.len() && hidden > 0).then_some(hidden);
        card(frame, *rect, s, job, view, more);
    }
}

fn card(
    frame: &mut Frame,
    area: Rect,
    s: &Subagent,
    job: &JobProgress,
    view: &View,
    more: Option<usize>,
) {
    let color = model_color(s.model.as_deref());
    let mark = match s.status {
        SubagentStatus::Running => spinner(view.frame),
        SubagentStatus::Done => "[✓]",
        SubagentStatus::Failed => "[x]",
    };
    let title = Line::from(vec![
        Span::styled(
            format!(" {} ", fit(&s.kind, area.width.saturating_sub(8) as usize)),
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{mark} "), Style::new().fg(color)),
    ]);
    let mut block = panel(String::new(), color).title(title);
    if let Some(n) = more {
        block = block.title_bottom(Line::from(dim(format!(" +{n} more "))).right_aligned());
    }
    let w = block.inner(area).width as usize;
    let end = s.ended_tick.unwrap_or(view.end.max(job.last_tick));
    let model = s.model.as_deref().map_or("—".into(), display_name);
    let lines = vec![
        Line::from(vec![
            Span::styled(model, Style::new().fg(color)),
            label(format!(" · {} tool calls", s.tool_calls)),
        ]),
        Line::from(Span::raw(fit(&s.description, w))),
        Line::from(Span::styled(
            sparkline(&s.marks, s.started_tick, end.max(s.started_tick + 1), w),
            Style::new().fg(color),
        )),
        Line::from(vec![
            label("ctx: isolated ("),
            Span::raw(tokens(s.tokens.context)),
            label(")"),
        ]),
        status_line(s, w),
    ];
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// `waiting…`, `[Bash: 22 passed]`, `[context discarded · lean]`, …
fn status_line(s: &Subagent, w: usize) -> Line<'static> {
    match (s.status, &s.last_outcome) {
        (SubagentStatus::Done, _) => Line::from(Span::styled(
            fit("[context discarded · lean]", w),
            Style::new().fg(GREEN),
        )),
        (SubagentStatus::Failed, _) => Line::from(Span::styled(
            fit(
                &format!("[failed: {}]", s.result.as_deref().unwrap_or("stopped")),
                w,
            ),
            Style::new().fg(RED),
        )),
        (SubagentStatus::Running, None) if s.marks.is_empty() => Line::from(dim("waiting…")),
        (SubagentStatus::Running, None) => Line::from(dim("working…")),
        (SubagentStatus::Running, Some(o)) => {
            let text = if o.text.is_empty() { "ok" } else { &o.text };
            Line::from(Span::styled(
                fit(&format!("[{}: {text}]", o.tool), w),
                Style::new().fg(if o.is_error { RED } else { MUTED }),
            ))
        }
    }
}

/// Subagents as one line each, for small windows.
pub fn list(frame: &mut Frame, area: Rect, view: &View) {
    let block = panel(" subagents ".into(), MUTED);
    let inner = block.inner(area);
    let w = inner.width as usize;
    let lines: Vec<Line> = match view.job.filter(|j| !j.subagents.is_empty()) {
        None => vec![Line::from(dim("none yet · the lead works alone"))],
        Some(job) => job
            .subagents
            .iter()
            .map(|s| {
                let color = model_color(s.model.as_deref());
                let mark = match s.status {
                    SubagentStatus::Running => spinner(view.frame),
                    SubagentStatus::Done => "[✓]",
                    SubagentStatus::Failed => "[x]",
                };
                let end = s.ended_tick.unwrap_or(view.end.max(job.last_tick));
                let head = format!(
                    "{mark} {:<12} {:<10} ",
                    fit(&s.kind, 12),
                    fit(&s.model.as_deref().map_or("—".into(), display_name), 10)
                );
                let rest = w.saturating_sub(head.chars().count() + 12);
                let mut spans = vec![
                    Span::styled(head, Style::new().fg(color)),
                    Span::styled(
                        sparkline(&s.marks, s.started_tick, end.max(s.started_tick + 1), 8),
                        Style::new().fg(color),
                    ),
                    label(format!(" {:>3} ", tokens(s.tokens.context))),
                ];
                spans.extend(status_line(s, rest).spans);
                Line::from(spans)
            })
            .collect(),
    };
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// `[✓] worker [ ] explorer [ ] reviewer`.
fn checklist(job: &JobProgress) -> Vec<Span<'static>> {
    let mut spans = vec![label("review + verify · ")];
    let tick = |done: bool| if done { "[✓] " } else { "[ ] " };
    for s in &job.subagents {
        let done = s.status != SubagentStatus::Running;
        let color = if s.status == SubagentStatus::Failed {
            RED
        } else if done {
            GREEN
        } else {
            MUTED
        };
        spans.push(Span::styled(
            format!("{}{} ", tick(done), s.kind),
            Style::new().fg(color),
        ));
    }
    if let Some(r) = &job.review {
        let done = matches!(r, ReviewState::Done { .. } | ReviewState::Failed { .. });
        spans.push(Span::styled(
            format!("{}reviewer", tick(done)),
            Style::new().fg(if done { GREEN } else { MUTED }),
        ));
    }
    spans
}

/// What the review says about handing the work back.
fn verdict(job: &JobProgress) -> Line<'static> {
    match &job.review {
        None => Line::from(dim("no review · press v before sending to add one")),
        Some(ReviewState::Requested) => Line::from(dim("review starts when the lead is done")),
        Some(ReviewState::Running { .. }) => {
            Line::from(label("reviewer checking the diff · no edits until go"))
        }
        Some(ReviewState::Done { review, .. }) => match review.verdict {
            ReviewVerdict::Pass => Line::from(vec![
                Span::styled(
                    format!("{} diffs verified", review.diffs_checked),
                    Style::new().fg(GREEN),
                ),
                label(" · no edits until go"),
            ]),
            ReviewVerdict::Concerns => Line::from(Span::styled(
                format!(
                    "{} concerns on {} diffs · look before you ship",
                    review.concerns.len(),
                    review.diffs_checked
                ),
                Style::new().fg(RED),
            )),
        },
        Some(ReviewState::Failed { error, .. }) => Line::from(Span::styled(
            format!("review failed: {error}"),
            Style::new().fg(RED),
        )),
    }
}

/// The "back to lead" box.
pub fn back_to_lead(frame: &mut Frame, area: Rect, view: &View) {
    let Some(job) = view.job else {
        let block = panel(" back to lead · main session ".into(), DIM);
        frame.render_widget(
            Paragraph::new(Line::from(dim(
                "results come back here, verified before you ship",
            )))
            .block(block),
            area,
        );
        return;
    };
    let block = panel(
        format!(" back to {} · main session ", display_name(&job.model)),
        ORANGE,
    );
    let lines = vec![Line::from(checklist(job)), verdict(job)];
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// The hand-back as one line, for small windows.
pub fn back_line(view: &View) -> Line<'static> {
    let Some(job) = view.job else {
        return Line::from(dim(" back to lead · nothing yet"));
    };
    let mut spans = vec![Span::styled(
        format!(" back to {} · ", display_name(&job.model)),
        Style::new().fg(ORANGE),
    )];
    spans.extend(checklist(job).into_iter().skip(1));
    spans.push(label("· "));
    spans.extend(verdict(job).spans);
    Line::from(spans)
}
