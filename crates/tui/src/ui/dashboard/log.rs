//! `tail -f session.log`: the job's log (and its review's), newest at the
//! bottom, with times and actors coloured.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::View;
use super::widgets::{dim, model_color, panel};
use crate::ui::{GREEN, LABEL, MUTED, RED};

/// Width of the time, actor and kind columns of a log line.
const TIME_W: usize = 6;
const ACTOR_W: usize = 11;
const KIND_W: usize = 12;

pub fn render(frame: &mut Frame, area: Rect, view: &View) {
    let block = panel(" tail -f session.log ".into(), MUTED);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(job) = view.job else {
        frame.render_widget(
            Paragraph::new(Line::from(dim("waiting for the first job"))),
            inner,
        );
        return;
    };
    let room = inner.height as usize;
    let review_log = view.review.map(|r| &r.log);
    let total = job.log.len() + review_log.map_or(0, |l| l.len());
    let skip = total.saturating_sub(room);
    let color_of = |actor: &str| -> Color {
        match actor {
            "lead" => model_color(Some(&job.model)),
            "reviewer" => view.review.map_or(LABEL, |r| model_color(Some(&r.model))),
            "swarmux" => GREEN,
            other => job
                .subagents
                .iter()
                .find(|s| s.kind == other)
                .map_or(MUTED, |s| model_color(s.model.as_deref())),
        }
    };
    let lines: Vec<Line> = job
        .log
        .iter()
        .chain(review_log.into_iter().flatten())
        .skip(skip)
        .map(|l| styled(l, &color_of))
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Splits `00:01 lead       text        hello` into coloured columns.
fn styled<'a>(line: &'a str, color_of: &dyn Fn(&str) -> Color) -> Line<'a> {
    let cut = |s: &'a str, n: usize| -> (&'a str, &'a str) {
        match s.char_indices().nth(n) {
            Some((i, _)) => s.split_at(i),
            None => (s, ""),
        }
    };
    let (time, rest) = cut(line, TIME_W);
    let (actor, rest) = cut(rest, ACTOR_W);
    let (kind, text) = cut(rest, KIND_W);
    let kind_color = match kind.trim() {
        "error" | "cancelled" => RED,
        "result" => GREEN,
        _ => LABEL,
    };
    let text_style = if text.starts_with("ERROR") {
        Style::new().fg(RED)
    } else {
        Style::new()
    };
    Line::from(vec![
        Span::styled(time, Style::new().fg(MUTED)),
        Span::styled(actor, Style::new().fg(color_of(actor.trim()))),
        Span::styled(kind, Style::new().fg(kind_color)),
        Span::styled(text, text_style),
    ])
}
