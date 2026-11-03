//! `tail -f session.log`: the job's log (and its review's), newest at the
//! bottom, with times and actors coloured.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::parts::{dim, fit, label};
use crate::theme::Theme;
use crate::ui::grid::chrome::{self, fingerprint};
use crate::ui::grid::{Panel, PanelId, ViewModel};
use crate::ui::look::Icon;

/// Width of the time, actor and kind columns of a log line.
const TIME_W: usize = 6;
const ACTOR_W: usize = 11;
const KIND_W: usize = 12;

pub struct Log;

impl Panel for Log {
    fn id(&self) -> PanelId {
        PanelId::Log
    }
    fn title(&self) -> &'static str {
        "tail -f session.log"
    }
    fn icon(&self) -> Icon {
        Icon::Log
    }
    fn min_size(&self) -> (u16, u16) {
        (24, 3)
    }
    fn priority(&self) -> u8 {
        2
    }

    fn render(&self, frame: &mut Frame, area: Rect, vm: &ViewModel, t: &Theme, _tick: u64) {
        let view = &vm.view;
        let lines_now =
            view.job.map_or(0, |j| j.log.len()) + view.review.map_or(0, |r| r.log.len());
        let block = chrome::block(
            self,
            vm,
            t,
            t.border,
            vec![chrome::title(" tail -f session.log ", t.muted)],
            Some(fingerprint(lines_now)),
        );
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let Some(job) = view.job else {
            frame.render_widget(
                Paragraph::new(Line::from(dim(t, "waiting for the first job"))),
                inner,
            );
            return;
        };
        let room = inner.height as usize;
        let review_log = view.review.map(|r| &r.log);
        let skip = lines_now.saturating_sub(room);
        let color_of = |actor: &str| -> Color {
            match actor {
                "lead" => t.model(Some(&job.model)),
                "reviewer" => view.review.map_or(t.reviewer, |r| t.model(Some(&r.model))),
                "swarmux" => t.guard,
                other => job
                    .subagents
                    .iter()
                    .find(|s| s.kind == other)
                    .map_or(t.muted, |s| t.model(s.model.as_deref())),
            }
        };
        let lines: Vec<Line> = job
            .log
            .iter()
            .chain(review_log.into_iter().flatten())
            .skip(skip)
            .map(|l| styled(l, &color_of, t))
            .collect();
        frame.render_widget(Paragraph::new(lines), inner);
    }

    fn summary(&self, vm: &ViewModel, t: &Theme, _tick: u64) -> Line<'static> {
        let last = vm
            .view
            .review
            .and_then(|r| r.log.back())
            .or_else(|| vm.view.job.and_then(|j| j.log.back()));
        match last {
            Some(l) => Line::from(vec![
                label(t, " log "),
                Span::styled(fit(l, 200), Style::new().fg(t.text)),
            ]),
            None => Line::from(dim(t, " log · waiting for the first job")),
        }
    }
}

/// Splits `00:01 lead       text        hello` into coloured columns.
fn styled<'a>(line: &'a str, color_of: &dyn Fn(&str) -> Color, t: &Theme) -> Line<'a> {
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
        "error" | "cancelled" => t.error,
        "result" => t.ok,
        _ => t.label,
    };
    let text_color = if text.starts_with("ERROR") {
        t.error
    } else {
        t.text
    };
    Line::from(vec![
        Span::styled(time, Style::new().fg(t.muted)),
        Span::styled(actor, Style::new().fg(color_of(actor.trim()))),
        Span::styled(kind, Style::new().fg(kind_color)),
        Span::styled(text, Style::new().fg(text_color)),
    ])
}
