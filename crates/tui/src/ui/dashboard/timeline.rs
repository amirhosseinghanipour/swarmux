//! The session timeline: one row per actor, marks where it was active, the
//! phases as the x-axis and a "now" cursor.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_domain::{Actor, JobStatus, Phase};

use super::View;
use super::widgets::{dim, fit, model_color, panel};
use crate::ui::{DIM, GREEN, LABEL, MUTED, ORANGE};

const LABEL_W: usize = 10;

struct Row {
    label: String,
    color: Color,
    marks: Vec<u32>,
}

fn rows(view: &View) -> Vec<Row> {
    let Some(job) = view.job else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    if let Some(r) = view.review {
        rows.push(Row {
            label: "reviewer".into(),
            color: model_color(Some(&r.model)),
            marks: r
                .lanes
                .iter()
                .flat_map(|l| l.marks.iter().map(|m| m + r.log_offset))
                .collect(),
        });
    }
    if let Some(lead) = job.lanes.iter().find(|l| l.actor == Actor::Lead) {
        rows.push(Row {
            label: "lead".into(),
            color: model_color(Some(&job.model)),
            marks: lead.marks.clone(),
        });
    }
    if !job.decision_marks.is_empty() {
        rows.push(Row {
            label: "guard".into(),
            color: GREEN,
            marks: job.decision_marks.clone(),
        });
    }
    for s in &job.subagents {
        let mut marks = Vec::with_capacity(s.marks.len() + 1);
        marks.push(s.started_tick);
        marks.extend(&s.marks);
        rows.push(Row {
            label: s.kind.clone(),
            color: model_color(s.model.as_deref()),
            marks,
        });
    }
    rows
}

pub fn render(frame: &mut Frame, area: Rect, view: &View) {
    let block = panel(" session timeline ".into(), MUTED);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(job) = view.job else {
        frame.render_widget(Paragraph::new(Line::from(dim("no job yet"))), inner);
        return;
    };
    let track = (inner.width as usize).saturating_sub(LABEL_W + 1);
    if track < 4 || inner.height == 0 {
        return;
    }
    let running = job.status == JobStatus::Running
        || view.review.is_some_and(|r| r.status == JobStatus::Running);
    let end = view.end.max(1);
    // Leave room right of "now" while things run.
    let span = if running { end + end / 8 + 2 } else { end };
    let x = |t: u32| (u64::from(t.min(span)) * (track as u64 - 1) / u64::from(span)) as usize;
    let now = x(end);

    let mut lines: Vec<Line> = Vec::new();
    let rows = rows(view);
    let room = (inner.height as usize).saturating_sub(1);
    for row in rows.iter().take(room) {
        let mut cells = vec![' '; track];
        for &m in &row.marks {
            cells[x(m)] = '┃';
        }
        let cursor = cells[now] == ' ';
        let before: String = cells[..now].iter().collect();
        let after: String = cells[now + 1..].iter().collect();
        let at = if cursor { '│' } else { cells[now] };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:<LABEL_W$} ", fit(&row.label, LABEL_W)),
                Style::new().fg(row.color),
            ),
            Span::styled(before, Style::new().fg(row.color)),
            Span::styled(
                at.to_string(),
                Style::new().fg(if cursor { DIM } else { row.color }),
            ),
            Span::styled(after, Style::new().fg(row.color)),
        ]));
    }
    lines.push(axis(job.phase_ticks, track, &x, now));
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Phase names at the time each was first reached, and the "now" mark.
fn axis(
    ticks: [Option<u32>; 7],
    track: usize,
    x: &dyn Fn(u32) -> usize,
    now: usize,
) -> Line<'static> {
    let mut cells = vec![' '; track];
    let mut lit = vec![false; track];
    let mut free_from = 0;
    for phase in Phase::ALL {
        let Some(t) = ticks[phase.index()] else {
            continue;
        };
        let name = phase.as_str();
        // Pull a label in from the right edge rather than drop it.
        let at = x(t).max(free_from).min(track.saturating_sub(name.len()));
        if at < free_from || at + name.len() > track {
            continue;
        }
        for (i, c) in name.chars().enumerate() {
            cells[at + i] = c;
            lit[at + i] = true;
        }
        free_from = at + name.len() + 1;
    }
    if cells[now] == ' ' {
        cells[now] = '▲';
    }
    let mut spans = vec![Span::styled(
        format!("{:<LABEL_W$} ", "steps"),
        Style::new().fg(LABEL),
    )];
    // Group runs of the same style into one span.
    let mut run = String::new();
    let mut run_lit = None;
    for (i, c) in cells.into_iter().enumerate() {
        let is_lit = lit[i];
        if run_lit.is_some_and(|l| l != is_lit) {
            spans.push(axis_span(std::mem::take(&mut run), run_lit == Some(true)));
        }
        run_lit = Some(is_lit);
        run.push(c);
    }
    spans.push(axis_span(run, run_lit == Some(true)));
    Line::from(spans)
}

fn axis_span(text: String, lit: bool) -> Span<'static> {
    Span::styled(text, Style::new().fg(if lit { LABEL } else { ORANGE }))
}
