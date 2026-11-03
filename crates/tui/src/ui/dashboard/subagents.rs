//! Subagents: the delegate line, one card each (or a list when space is
//! short), and the hand-back to the lead with the review's verdict.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph},
};
use swarmux_domain::{
    JobProgress, ReviewState, ReviewVerdict, Subagent, SubagentStatus, display_name,
};

use super::parts::{buckets, dim, fit, label, sparkline, text, tokens};
use crate::theme::Theme;
use crate::ui::grid::chrome::{self, fingerprint};
use crate::ui::grid::{Panel, PanelId, ViewModel};
use crate::ui::look::Icon;
use crate::ui::widgets::{SparkStyle, Sparkline};

/// Narrowest card; more subagents than fit are summed up in the last one.
const CARD_MIN_W: u16 = 24;
/// Rows a card needs.
const CARD_H: u16 = 7;

/// `: delegate · 3 subagents · parallel · effort — :` and a ▼ per card.
pub struct Delegate;

impl Panel for Delegate {
    fn id(&self) -> PanelId {
        PanelId::Delegate
    }
    fn title(&self) -> &'static str {
        "delegate"
    }
    fn icon(&self) -> Icon {
        Icon::Subagents
    }
    fn min_size(&self) -> (u16, u16) {
        (20, 2)
    }
    fn priority(&self) -> u8 {
        8
    }
    fn boxed(&self) -> bool {
        false
    }
    fn render(&self, frame: &mut Frame, area: Rect, vm: &ViewModel, t: &Theme, tick: u64) {
        let [line, arrows] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
        frame.render_widget(self.summary(vm, t, tick).centered(), line);
        let Some(job) = vm.view.job.filter(|j| !j.subagents.is_empty()) else {
            return;
        };
        for col in card_areas(arrows, job.subagents.len()) {
            let x = col.x + col.width / 2;
            frame.render_widget(
                Span::styled("▼", Style::new().fg(t.dim)),
                Rect::new(x, arrows.y, 1, 1),
            );
        }
    }
    fn summary(&self, vm: &ViewModel, t: &Theme, _tick: u64) -> Line<'static> {
        let Some(job) = vm.view.job.filter(|j| !j.subagents.is_empty()) else {
            return Line::from(dim(t, ": no subagents yet · the lead works alone :"));
        };
        let running = job.subagents_running();
        let mode = match running {
            0 => "merged",
            1 => "single",
            _ => "parallel",
        };
        Line::from(dim(
            t,
            format!(
                ": delegate · {} subagents · {mode} · effort — :",
                job.subagents.len()
            ),
        ))
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

/// One card per subagent, or a list when there is no room for cards.
pub struct Subagents;

impl Panel for Subagents {
    fn id(&self) -> PanelId {
        PanelId::Subagents
    }
    fn title(&self) -> &'static str {
        "subagents"
    }
    fn icon(&self) -> Icon {
        Icon::Subagents
    }
    fn min_size(&self) -> (u16, u16) {
        (24, 3)
    }
    fn priority(&self) -> u8 {
        2
    }
    fn preferred_height(&self, vm: &ViewModel) -> Option<u16> {
        Some(vm.view.job.map_or(1, |j| j.subagents.len().max(1)) as u16 + 2)
    }
    fn render(&self, frame: &mut Frame, area: Rect, vm: &ViewModel, t: &Theme, _tick: u64) {
        if area.height >= CARD_H {
            cards(frame, area, vm, t);
        } else {
            list(frame, area, vm, t);
        }
    }
    fn summary(&self, vm: &ViewModel, t: &Theme, _tick: u64) -> Line<'static> {
        let Some(job) = vm.view.job.filter(|j| !j.subagents.is_empty()) else {
            return Line::from(dim(t, " subagents · none yet"));
        };
        let mut spans = vec![label(t, " subagents ")];
        for s in &job.subagents {
            let color = t.model(s.model.as_deref());
            spans.push(Span::styled(
                format!("{} {} ", mark(s, vm), s.kind),
                Style::new().fg(color),
            ));
        }
        Line::from(spans)
    }
}

fn mark(s: &Subagent, vm: &ViewModel) -> String {
    match s.status {
        SubagentStatus::Running => format!("[{}]", vm.spin()),
        SubagentStatus::Done => "[✓]".into(),
        SubagentStatus::Failed => "[x]".into(),
    }
}

fn cards(frame: &mut Frame, area: Rect, vm: &ViewModel, t: &Theme) {
    let Some(job) = vm.view.job.filter(|j| !j.subagents.is_empty()) else {
        let block = Block::bordered()
            .border_type(BorderType::LightDoubleDashed)
            .border_style(Style::new().fg(t.dim))
            .style(Style::new().bg(t.panel_bg));
        frame.render_widget(
            Paragraph::new(vec![
                Line::raw(""),
                Line::from(dim(t, "subagents appear here when the lead delegates")),
                Line::from(dim(
                    t,
                    "(Task / Agent tool) · each gets an isolated context",
                )),
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
        card(frame, *rect, s, job, vm, t, more);
    }
}

fn card(
    frame: &mut Frame,
    area: Rect,
    s: &Subagent,
    job: &JobProgress,
    vm: &ViewModel,
    t: &Theme,
    more: Option<usize>,
) {
    let color = t.model(s.model.as_deref());
    let title = vec![
        chrome::title(
            format!(" {} ", fit(&s.kind, area.width.saturating_sub(10) as usize)),
            color,
        ),
        Span::styled(format!("{} ", mark(s, vm)), Style::new().fg(color)),
    ];
    let mut block = chrome::framed(
        PanelId::Subagents,
        vm.icon(Icon::Subagents),
        vm,
        t,
        color,
        title,
        None,
        chrome::Border::Rounded,
    );
    if let Some(n) = more {
        block = block.title_bottom(Line::from(dim(t, format!(" +{n} more "))).right_aligned());
    }
    let inner = block.inner(area);
    let w = inner.width as usize;
    let end = s.ended_tick.unwrap_or(vm.view.end.max(job.last_tick));
    let model = s.model.as_deref().map_or("—".into(), display_name);
    let lines = vec![
        Line::from(vec![
            Span::styled(model, Style::new().fg(color)),
            label(t, format!(" · {} tool calls", s.tool_calls)),
        ]),
        Line::from(text(t, fit(&s.description, w))),
        Line::raw(""),
        Line::from(vec![
            label(t, "ctx: isolated ("),
            text(t, tokens(s.tokens.context)),
            label(t, ")"),
        ]),
        status_line(s, w, t),
    ];
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines), inner);
    // Activity as a braille graph on the third row.
    if inner.height >= 3 {
        let data = buckets(&s.marks, s.started_tick, end.max(s.started_tick + 1), w * 2);
        let stops = [t.dim, color];
        let row = Rect {
            y: inner.y + 2,
            height: 1,
            ..inner
        };
        frame.render_widget(
            Sparkline::new(&data, &stops).style(SparkStyle::Braille),
            row,
        );
    }
}

/// `waiting…`, `[Bash: 22 passed]`, `[context discarded · lean]`, …
fn status_line(s: &Subagent, w: usize, t: &Theme) -> Line<'static> {
    match (s.status, &s.last_outcome) {
        (SubagentStatus::Done, _) => Line::from(Span::styled(
            fit("[context discarded · lean]", w),
            Style::new().fg(t.ok),
        )),
        (SubagentStatus::Failed, _) => Line::from(Span::styled(
            fit(
                &format!("[failed: {}]", s.result.as_deref().unwrap_or("stopped")),
                w,
            ),
            Style::new().fg(t.error),
        )),
        (SubagentStatus::Running, None) if s.marks.is_empty() => Line::from(dim(t, "waiting…")),
        (SubagentStatus::Running, None) => Line::from(dim(t, "working…")),
        (SubagentStatus::Running, Some(o)) => {
            let text = if o.text.is_empty() { "ok" } else { &o.text };
            Line::from(Span::styled(
                fit(&format!("[{}: {text}]", o.tool), w),
                Style::new().fg(if o.is_error { t.error } else { t.muted }),
            ))
        }
    }
}

/// Subagents as one line each, for small windows.
fn list(frame: &mut Frame, area: Rect, vm: &ViewModel, t: &Theme) {
    let block = chrome::block(
        &Subagents,
        vm,
        t,
        t.border,
        vec![chrome::title(" subagents ", t.muted)],
        None,
    );
    let inner = block.inner(area);
    let w = inner.width as usize;
    let lines: Vec<Line> = match vm.view.job.filter(|j| !j.subagents.is_empty()) {
        None => vec![Line::from(dim(t, "none yet · the lead works alone"))],
        Some(job) => job
            .subagents
            .iter()
            .map(|s| {
                let color = t.model(s.model.as_deref());
                let end = s.ended_tick.unwrap_or(vm.view.end.max(job.last_tick));
                let head = format!(
                    "{} {:<12} {:<10} ",
                    mark(s, vm),
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
                    label(t, format!(" {:>3} ", tokens(s.tokens.context))),
                ];
                spans.extend(status_line(s, rest, t).spans);
                Line::from(spans)
            })
            .collect(),
    };
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// `[✓] worker [ ] explorer [ ] reviewer`.
fn checklist(job: &JobProgress, t: &Theme) -> Vec<Span<'static>> {
    let mut spans = vec![label(t, "review + verify · ")];
    let tick = |done: bool| if done { "[✓] " } else { "[ ] " };
    for s in &job.subagents {
        let done = s.status != SubagentStatus::Running;
        let color = if s.status == SubagentStatus::Failed {
            t.error
        } else if done {
            t.ok
        } else {
            t.muted
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
            Style::new().fg(if done { t.ok } else { t.muted }),
        ));
    }
    spans
}

/// What the review says about handing the work back.
fn verdict(job: &JobProgress, t: &Theme) -> Line<'static> {
    match &job.review {
        None => Line::from(dim(t, "no review · press v before sending to add one")),
        Some(ReviewState::Requested) => Line::from(dim(t, "review starts when the lead is done")),
        Some(ReviewState::Running { .. }) => {
            Line::from(label(t, "reviewer checking the diff · no edits until go"))
        }
        Some(ReviewState::Done { review, .. }) => match review.verdict {
            ReviewVerdict::Pass => Line::from(vec![
                Span::styled(
                    format!("{} diffs verified", review.diffs_checked),
                    Style::new().fg(t.ok),
                ),
                label(t, " · no edits until go"),
            ]),
            ReviewVerdict::Concerns => Line::from(Span::styled(
                format!(
                    "{} concerns on {} diffs · look before you ship",
                    review.concerns.len(),
                    review.diffs_checked
                ),
                Style::new().fg(t.error),
            )),
        },
        Some(ReviewState::Failed { error, .. }) => Line::from(Span::styled(
            format!("review failed: {error}"),
            Style::new().fg(t.error),
        )),
        Some(ReviewState::Skipped { why }) => Line::from(dim(t, format!("review skipped · {why}"))),
    }
}

/// The "back to lead" box.
pub struct Back;

impl Panel for Back {
    fn id(&self) -> PanelId {
        PanelId::Back
    }
    fn title(&self) -> &'static str {
        "back to lead"
    }
    fn icon(&self) -> Icon {
        Icon::Back
    }
    fn min_size(&self) -> (u16, u16) {
        (30, 4)
    }
    fn priority(&self) -> u8 {
        5
    }
    fn render(&self, frame: &mut Frame, area: Rect, vm: &ViewModel, t: &Theme, _tick: u64) {
        let Some(job) = vm.view.job else {
            let block = chrome::block(
                self,
                vm,
                t,
                t.dim,
                vec![chrome::title(" back to lead · main session ", t.dim)],
                None,
            );
            frame.render_widget(
                Paragraph::new(Line::from(dim(
                    t,
                    "results come back here, verified before you ship",
                )))
                .block(block),
                area,
            );
            return;
        };
        let done = job
            .subagents
            .iter()
            .filter(|s| s.status != SubagentStatus::Running)
            .count();
        let block = chrome::block(
            self,
            vm,
            t,
            t.lead,
            vec![chrome::title(
                format!(" back to {} · main session ", display_name(&job.model)),
                t.lead,
            )],
            Some(fingerprint(done)),
        );
        let lines = vec![Line::from(checklist(job, t)), verdict(job, t)];
        frame.render_widget(Paragraph::new(lines).block(block), area);
    }

    /// The hand-back as one line, for small windows.
    fn summary(&self, vm: &ViewModel, t: &Theme, _tick: u64) -> Line<'static> {
        let Some(job) = vm.view.job else {
            return Line::from(dim(t, " back to lead · nothing yet"));
        };
        let mut spans = vec![Span::styled(
            format!(" back to {} · ", display_name(&job.model)),
            Style::new().fg(t.lead),
        )];
        spans.extend(checklist(job, t).into_iter().skip(1));
        spans.push(label(t, "· "));
        spans.extend(verdict(job, t).spans);
        Line::from(spans)
    }
}
