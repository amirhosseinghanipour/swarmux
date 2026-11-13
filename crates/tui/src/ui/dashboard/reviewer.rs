//! The reviewer: a silent auditor that checks a job before it is done.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_domain::{JobStatus, Phase, ReviewState, ReviewVerdict, display_name};

use super::View;
use super::header::reviewer_model;
use super::parts::{buckets, dim, fit, label, text, tokens};
use crate::theme::Theme;
use crate::ui::grid::chrome::{self, Border, fingerprint};
use crate::ui::grid::{Panel, PanelId, ViewModel};
use crate::ui::look::Icon;
use crate::ui::widgets::{SparkStyle, Sparkline};

pub struct Reviewer;

impl Panel for Reviewer {
    fn id(&self) -> PanelId {
        PanelId::Reviewer
    }
    fn title(&self) -> &'static str {
        "reviewer"
    }
    fn icon(&self) -> Icon {
        Icon::Reviewer
    }
    fn min_size(&self) -> (u16, u16) {
        (28, 6)
    }
    fn priority(&self) -> u8 {
        3
    }

    /// The full panel with its three moments, counters and activity.
    fn render(&self, frame: &mut Frame, area: Rect, vm: &ViewModel, t: &Theme, _tick: u64) {
        let (app, view) = (vm.app, &vm.view);
        let model = reviewer_model(app, view);
        let running = view.review.is_some_and(|r| r.status == JobStatus::Running);
        let color = model.as_deref().map_or(t.muted, |m| t.model(Some(m)));
        let mut title = vec![chrome::title(
            format!(
                " REVIEWER · {} · adversary ",
                model.as_deref().map_or("off".into(), display_name)
            ),
            color,
        )];
        if running {
            title.push(Span::styled(
                format!("[{}] ", vm.spin()),
                Style::new().fg(color),
            ));
        }
        let state = view.job.and_then(|j| j.review.as_ref()).map(state_name);
        let block = chrome::framed(
            self.id(),
            vm.icon(self.icon()),
            vm,
            t,
            color,
            title,
            Some(fingerprint(state)),
            Border::Dashed,
        );
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let w = inner.width as usize;
        let phase = view.job.map(|j| j.phase());
        let review = view.job.and_then(|j| j.review.as_ref());
        let mut lines = vec![Line::from(match (review, view.job) {
            (Some(_), _) => label(t, "on call · silent auditor"),
            (None, None) if app.review_next() => label(t, "on call for the next job"),
            _ => dim(
                t,
                format!(
                    "off · press {} before sending",
                    crate::hints::key_or_name(app, crate::screens::dashboard::TOGGLE_REVIEW)
                ),
            ),
        })];
        lines.push(Line::raw(""));

        lines.push(section(
            t,
            "before a plan",
            matches!(phase, Some(Phase::Plan | Phase::Fork)),
        ));
        for s in [
            "checks the diff against the task",
            "reads only · no edits · no session",
        ] {
            lines.push(Line::from(dim(
                t,
                format!("  {}", fit(s, w.saturating_sub(2))),
            )));
        }
        lines.push(Line::raw(""));

        let stops = view.job.map_or(&[][..], |j| j.loop_stops.as_slice());
        lines.push(section(t, "error again", !stops.is_empty()));
        if stops.is_empty() {
            lines.push(Line::from(dim(t, "  no repeated failures")));
        }
        for reason in stops.iter().rev().take(2) {
            let tool = reason
                .strip_prefix("loop guard: ")
                .and_then(|r| r.split(' ').next())
                .unwrap_or("a tool");
            lines.push(Line::from(vec![
                Span::styled(format!("  looping on {tool}?"), Style::new().fg(t.error)),
                label(t, " » stop symptom fix"),
            ]));
        }
        lines.push(Line::raw(""));

        lines.push(section(t, "before done", phase == Some(Phase::Audit)));
        lines.extend(verdict_lines(review, view, vm, t, w));

        let run = view.review;
        let footer = Line::from(vec![
            label(t, "calls   "),
            text(
                t,
                run.map_or("—".into(), |r| r.lead_tokens.turns.to_string()),
            ),
            label(t, "   tokens  "),
            text(t, run.map_or("—".into(), |r| tokens(r.lead_tokens.seen))),
        ]);
        // Calls and tokens, then a braille activity graph on the last rows.
        let graph_h: u16 = if inner.height >= 16 { 2 } else { 1 };
        let room = (inner.height as usize).saturating_sub(graph_h as usize);
        lines.truncate(room.saturating_sub(2));
        while lines.len() + 1 < room {
            lines.push(Line::raw(""));
        }
        lines.push(footer);
        frame.render_widget(Paragraph::new(lines), inner);
        if let Some(r) = run
            && inner.height > graph_h
        {
            let marks = r.lanes.first().map_or(&[][..], |l| l.marks.as_slice());
            let data = buckets(marks, 0, r.last_tick.max(1), w * 2);
            let graph = Rect {
                y: inner.bottom() - graph_h,
                height: graph_h,
                ..inner
            };
            let stops = [t.dim, color];
            frame.render_widget(
                Sparkline::new(&data, &stops).style(SparkStyle::Braille),
                graph,
            );
        }
    }

    /// `REVIEWER · opus · on call · » 0 concerns · 3 diffs approved` for small windows.
    fn summary(&self, vm: &ViewModel, t: &Theme, _tick: u64) -> Line<'static> {
        let (app, view) = (vm.app, &vm.view);
        let model = reviewer_model(app, view);
        let color = model.as_deref().map_or(t.muted, |m| t.model(Some(m)));
        let mut spans = vec![Span::styled(
            format!(
                " REVIEWER · {} · ",
                model.as_deref().map_or("off".into(), display_name)
            ),
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        )];
        let review = view.job.and_then(|j| j.review.as_ref());
        spans.push(match review {
            None if view.job.is_none() && app.review_next() => label(t, "on call for the next job"),
            None => dim(
                t,
                format!(
                    "off · press {} before sending",
                    crate::hints::key_or_name(app, crate::screens::dashboard::TOGGLE_REVIEW)
                ),
            ),
            Some(ReviewState::Requested) => label(t, "on call · waits for the job"),
            Some(ReviewState::Running { .. }) => label(t, format!("reviewing [{}]", vm.spin())),
            Some(ReviewState::Done { review, .. }) => {
                let n = review.concerns.len();
                Span::styled(
                    format!(
                        "» {n} concerns · {} diffs {}",
                        review.diffs_checked,
                        if n == 0 {
                            "approved [OK]"
                        } else {
                            "checked [!!]"
                        }
                    ),
                    Style::new().fg(if n == 0 { t.ok } else { t.error }),
                )
            }
            Some(ReviewState::Failed { error, .. }) => Span::styled(
                format!("review failed: {}", fit(error, 60)),
                Style::new().fg(t.error),
            ),
            Some(ReviewState::Skipped { why }) => dim(t, format!("skipped · {}", fit(why, 60))),
        });
        Line::from(spans)
    }
}

fn state_name(r: &ReviewState) -> &'static str {
    match r {
        ReviewState::Requested => "requested",
        ReviewState::Running { .. } => "running",
        ReviewState::Done { .. } => "done",
        ReviewState::Failed { .. } => "failed",
        ReviewState::Skipped { .. } => "skipped",
    }
}

/// `<before done>`, highlighted while it is the moment that matters.
fn section(t: &Theme, name: &str, active: bool) -> Line<'static> {
    let style = if active {
        Style::new()
            .fg(t.on_accent)
            .bg(t.lead)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(t.label)
    };
    Line::from(Span::styled(format!("<{name}>"), style))
}

/// What the reviewer said, or where it stands.
fn verdict_lines(
    review: Option<&ReviewState>,
    view: &View,
    vm: &ViewModel,
    t: &Theme,
    w: usize,
) -> Vec<Line<'static>> {
    let cost = |c: f64| dim(t, format!("  cost ${c:.4}"));
    match review {
        None => vec![Line::from(dim(t, "  not requested"))],
        Some(ReviewState::Requested) => vec![Line::from(dim(t, "  waits for the job to finish"))],
        Some(ReviewState::Skipped { why }) => {
            vec![Line::from(dim(
                t,
                format!("  skipped: {}", fit(why, w.saturating_sub(11))),
            ))]
        }
        Some(ReviewState::Running { .. }) => {
            let spin = match view.review.map(|r| r.status) {
                Some(JobStatus::Running) => format!("[{}]", vm.spin()),
                _ => "[·]".into(),
            };
            vec![Line::from(vec![
                label(t, "  reviewing the diff "),
                Span::styled(spin, Style::new().fg(t.lead)),
            ])]
        }
        Some(ReviewState::Done {
            review, cost_usd, ..
        }) => {
            let n = review.concerns.len();
            let mut lines = vec![Line::from(vec![
                label(t, "  » "),
                Span::styled(
                    format!("{n} concern{}", if n == 1 { "" } else { "s" }),
                    Style::new().fg(if n == 0 { t.ok } else { t.error }),
                ),
            ])];
            for c in review.concerns.iter().take(3) {
                lines.push(Line::from(Span::styled(
                    format!("    {}", fit(c, w.saturating_sub(4))),
                    Style::new().fg(t.error),
                )));
            }
            let (word, mark, color) = match review.verdict {
                ReviewVerdict::Pass => ("approved", "[OK]", t.ok),
                ReviewVerdict::Concerns => ("checked", "[!!]", t.error),
            };
            lines.push(Line::from(vec![
                text(t, format!("  {} diffs {word} ", review.diffs_checked)),
                Span::styled(mark, Style::new().fg(color).add_modifier(Modifier::BOLD)),
            ]));
            lines.push(Line::from(cost(*cost_usd)));
            lines
        }
        Some(ReviewState::Failed {
            error, cost_usd, ..
        }) => vec![
            Line::from(Span::styled(
                format!("  review failed: {}", fit(error, w.saturating_sub(17))),
                Style::new().fg(t.error),
            )),
            Line::from(cost(*cost_usd)),
        ],
    }
}
