//! The answer pane: the agent's final report as readable text, or the
//! working-tree diff in its place (`d`). Scrolls with `J` and `K`.

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_domain::JobStatus;

use super::parts::{diff_style, markdown, wrap};
use crate::app::App;
use crate::focus::Focus;
use crate::focus::stream::diff_lines;
use crate::ui::dashboard::{dim, fit};
use crate::ui::look::Icon;
use crate::ui::mission::{glyph, panel};

pub fn render(frame: &mut Frame, area: Rect, app: &App, f: &Focus) {
    let t = &app.look.theme;
    let state = &app.focus_state;
    let diff = state.diff.as_ref();
    let (title, icon) = if diff.is_some() {
        ("working tree diff", Icon::Files)
    } else {
        ("answer", Icon::Result)
    };
    let hint = if diff.is_some() {
        " d back to the answer "
    } else {
        " J/K scroll · d diff "
    };
    let block = panel(
        t,
        glyph(app, icon),
        title,
        Line::from(dim(t, hint)),
        t.border,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 12 || inner.height == 0 {
        return;
    }
    let w = usize::from(inner.width);

    let lines: Vec<Line<'static>> = if let Some(diff) = diff {
        let text = diff.join("\n");
        diff_lines(&text)
            .into_iter()
            .map(|(kind, line)| {
                let style = diff_style(t, kind);
                let line = fit(line, w);
                let pad = w.saturating_sub(line.chars().count());
                Line::from(Span::styled(format!("{line}{}", " ".repeat(pad)), style))
            })
            .collect()
    } else if let Some(answer) = &f.answer {
        let mut lines = markdown(t, answer, w);
        if f.who.status == JobStatus::Failed {
            lines.insert(
                0,
                Line::styled("it failed; its last words:", Style::new().fg(t.error)),
            );
        }
        lines
    } else {
        let hint = match f.who.status {
            JobStatus::Queued => "queued: the answer comes when the job has run",
            JobStatus::Running => "the answer appears here when the job finishes",
            _ => "this job left no answer",
        };
        wrap(&[(hint.to_string(), Style::new().fg(t.dim))], w)
    };

    let height = usize::from(inner.height);
    let max = lines.len().saturating_sub(height);
    state.max_answer_scroll.set(max);
    let from = state.answer_scroll.min(max);
    let shown: Vec<Line> = lines.into_iter().skip(from).take(height).collect();
    frame.render_widget(Paragraph::new(shown), inner);
}
