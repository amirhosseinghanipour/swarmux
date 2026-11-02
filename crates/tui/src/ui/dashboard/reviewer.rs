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
use super::widgets::{dashed, dim, fit, label, model_color, sparkline, spinner, tokens};
use crate::app::App;
use crate::ui::{BG, GREEN, LABEL, MUTED, ORANGE, RED};

/// The full panel with its three moments, counters and activity.
pub fn panel(frame: &mut Frame, area: Rect, app: &App, view: &View) {
    let model = reviewer_model(app, view);
    let running = view.review.is_some_and(|r| r.status == JobStatus::Running);
    let mut title = format!(
        " REVIEWER · {} · adversary ",
        model.as_deref().map_or("off".into(), display_name)
    );
    if running {
        title.push_str(spinner(view.frame));
        title.push(' ');
    }
    let color = match &model {
        Some(m) => model_color(Some(m)),
        None => MUTED,
    };
    let block = dashed(title, color);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let w = inner.width as usize;
    let phase = view.job.map(|j| j.phase());
    let review = view.job.and_then(|j| j.review.as_ref());
    let mut lines = vec![Line::from(match (review, view.job) {
        (Some(_), _) => label("on call · silent auditor"),
        (None, None) if app.review_next() => label("on call for the next job"),
        _ => dim("off · press v before sending"),
    })];
    lines.push(Line::raw(""));

    lines.push(section(
        "before a plan",
        matches!(phase, Some(Phase::Plan | Phase::Fork)),
    ));
    for t in [
        "checks the diff against the task",
        "reads only · no edits · no session",
    ] {
        lines.push(Line::from(dim(format!(
            "  {}",
            fit(t, w.saturating_sub(2))
        ))));
    }
    lines.push(Line::raw(""));

    let stops = view.job.map_or(&[][..], |j| j.loop_stops.as_slice());
    lines.push(section("error again", !stops.is_empty()));
    if stops.is_empty() {
        lines.push(Line::from(dim("  no repeated failures")));
    }
    for reason in stops.iter().rev().take(2) {
        let tool = reason
            .strip_prefix("loop guard: ")
            .and_then(|r| r.split(' ').next())
            .unwrap_or("a tool");
        lines.push(Line::from(vec![
            Span::styled(format!("  looping on {tool}?"), Style::new().fg(RED)),
            label(" » stop symptom fix"),
        ]));
    }
    lines.push(Line::raw(""));

    lines.push(section("before done", phase == Some(Phase::Audit)));
    lines.extend(verdict_lines(review, view, w));

    let run = view.review;
    let footer = [
        Line::from(vec![
            label("calls   "),
            Span::raw(run.map_or("—".into(), |r| r.lead_tokens.turns.to_string())),
            label("   tokens  "),
            Span::raw(run.map_or("—".into(), |r| tokens(r.lead_tokens.seen))),
        ]),
        Line::from(Span::styled(
            run.map_or_else(String::new, |r| {
                let marks = r.lanes.first().map_or(&[][..], |l| l.marks.as_slice());
                sparkline(marks, 0, r.last_tick.max(1), w)
            }),
            Style::new().fg(color),
        )),
    ];
    let room = inner.height as usize;
    lines.truncate(room.saturating_sub(footer.len() + 1));
    while lines.len() + footer.len() < room {
        lines.push(Line::raw(""));
    }
    lines.extend(footer);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// `<before done>`, highlighted while it is the moment that matters.
fn section(name: &str, active: bool) -> Line<'static> {
    let style = if active {
        Style::new().fg(BG).bg(ORANGE).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(LABEL)
    };
    Line::from(Span::styled(format!("<{name}>"), style))
}

/// What the reviewer said, or where it stands.
fn verdict_lines(review: Option<&ReviewState>, view: &View, w: usize) -> Vec<Line<'static>> {
    let cost = |c: f64| dim(format!("  cost ${c:.4}"));
    match review {
        None => vec![Line::from(dim("  not requested"))],
        Some(ReviewState::Requested) => vec![Line::from(dim("  waits for the job to finish"))],
        Some(ReviewState::Running { .. }) => {
            let spin = match view.review.map(|r| r.status) {
                Some(JobStatus::Running) => spinner(view.frame),
                _ => "[·]",
            };
            vec![Line::from(vec![
                label("  reviewing the diff "),
                Span::styled(spin, Style::new().fg(ORANGE)),
            ])]
        }
        Some(ReviewState::Done {
            review, cost_usd, ..
        }) => {
            let n = review.concerns.len();
            let mut lines = vec![Line::from(vec![
                label("  » "),
                Span::styled(
                    format!("{n} concern{}", if n == 1 { "" } else { "s" }),
                    Style::new().fg(if n == 0 { GREEN } else { RED }),
                ),
            ])];
            for c in review.concerns.iter().take(3) {
                lines.push(Line::from(Span::styled(
                    format!("    {}", fit(c, w.saturating_sub(4))),
                    Style::new().fg(RED),
                )));
            }
            let (word, mark, color) = match review.verdict {
                ReviewVerdict::Pass => ("approved", "[OK]", GREEN),
                ReviewVerdict::Concerns => ("checked", "[!!]", RED),
            };
            lines.push(Line::from(vec![
                Span::raw(format!("  {} diffs {word} ", review.diffs_checked)),
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
                Style::new().fg(RED),
            )),
            Line::from(cost(*cost_usd)),
        ],
    }
}

/// `REVIEWER · opus · on call · » 0 concerns · 3 diffs approved` for small windows.
pub fn one_line(app: &App, view: &View) -> Line<'static> {
    let model = reviewer_model(app, view);
    let color = model.as_deref().map_or(MUTED, |m| model_color(Some(m)));
    let mut spans = vec![Span::styled(
        format!(
            " REVIEWER · {} · ",
            model.as_deref().map_or("off".into(), display_name)
        ),
        Style::new().fg(color).add_modifier(Modifier::BOLD),
    )];
    let review = view.job.and_then(|j| j.review.as_ref());
    spans.push(match review {
        None if view.job.is_none() && app.review_next() => label("on call for the next job"),
        None => dim("off · press v before sending"),
        Some(ReviewState::Requested) => label("on call · waits for the job"),
        Some(ReviewState::Running { .. }) => label(format!("reviewing {}", spinner(view.frame))),
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
                Style::new().fg(if n == 0 { GREEN } else { RED }),
            )
        }
        Some(ReviewState::Failed { error, .. }) => Span::styled(
            format!("review failed: {}", fit(error, 60)),
            Style::new().fg(RED),
        ),
    });
    Line::from(spans)
}
