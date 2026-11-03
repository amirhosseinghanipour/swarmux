//! The strips above the stream: the situation card, the gate lane with the
//! checkpoints and the recovery trail, and the timeline.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::parts::{meter, tone};
use crate::app::App;
use crate::focus::{Confidence, Focus, LaneKind};
use crate::ui::dashboard::{buckets, dim, fit, label};
use crate::ui::look::Icon;
use crate::ui::mission::{glyph, panel};

/// Width of the left label column of the cards.
const LABEL_W: usize = 9;

fn labelled(app: &App, name: &str, body: Vec<Span<'static>>) -> Line<'static> {
    let t = &app.look.theme;
    let mut spans = vec![label(t, format!("{name:<LABEL_W$}"))];
    spans.extend(body);
    Line::from(spans)
}

/// `doing · because · next · confidence · unsure about`.
pub fn sat(frame: &mut Frame, area: Rect, app: &App, f: &Focus) {
    let t = &app.look.theme;
    let s = &f.sat;
    let conf_color = match s.confidence {
        Confidence::Low => t.error,
        Confidence::Medium => t.warn,
        Confidence::High => t.ok,
    };
    let meta = Line::from(Span::styled(
        format!(" confidence {} ", s.confidence.word()),
        Style::new().fg(conf_color),
    ));
    let block = panel(t, glyph(app, Icon::Context), "situation", meta, t.border);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 20 || inner.height == 0 {
        return;
    }
    let w = usize::from(inner.width).saturating_sub(LABEL_W);
    let text = |s: &str| vec![Span::styled(fit(s, w), Style::new().fg(t.text))];
    let sure = vec![
        Span::styled(
            meter(app, s.confidence.cells(), 5),
            Style::new().fg(conf_color),
        ),
        Span::styled(
            format!(" {}", s.confidence.word()),
            Style::new().fg(conf_color),
        ),
    ];
    let unsure = if s.unsure.is_empty() {
        vec![Span::styled("nothing flagged", Style::new().fg(t.dim))]
    } else {
        vec![Span::styled(
            fit(&s.unsure.join(" · "), w),
            Style::new().fg(t.warn),
        )]
    };
    let lines = vec![
        labelled(app, "doing", text(&s.doing)),
        labelled(app, "because", text(&s.because)),
        labelled(app, "next", text(&s.next)),
        labelled(app, "sure", sure),
        labelled(app, "unsure", unsure),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Gate chips on the first line; checkpoints and the recovery trail on the
/// second.
pub fn lane(frame: &mut Frame, area: Rect, app: &App, f: &Focus) {
    let t = &app.look.theme;
    let meta = Line::from(dim(t, " checks · checkpoints · recovery "));
    let block = panel(t, glyph(app, Icon::Guard), "gate", meta, t.border);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 20 || inner.height == 0 {
        return;
    }
    let w = usize::from(inner.width);

    let chips: Vec<Span> = if f.chips.is_empty() {
        vec![dim(t, "the gate runs when the agent stops")]
    } else {
        f.chips
            .iter()
            .flat_map(|c| {
                let (icon, color) = tone(t, c.tone);
                [
                    Span::styled(glyph(app, icon), Style::new().fg(color)),
                    Span::styled(format!(" {}  ", c.text), Style::new().fg(t.text)),
                ]
            })
            .collect()
    };
    frame.render_widget(Line::from(chips), Rect { height: 1, ..inner });
    if inner.height < 2 {
        return;
    }

    let mut spans = vec![label(t, "checkpoints ")];
    let cp = &f.checkpoints;
    if cp.dots.is_empty() {
        spans.push(dim(t, "none yet"));
    } else {
        for (i, dot) in cp.dots.iter().enumerate() {
            let selected = app.focus_state.checkpoint == Some(i);
            spans.push(Span::styled(
                format!("{} ", glyph(app, Icon::Checkpoint)),
                if selected {
                    Style::new().fg(t.title).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(t.muted)
                },
            ));
            if selected {
                spans.push(Span::styled(
                    format!("{} ", fit(&dot.label, 24)),
                    Style::new().fg(t.title),
                ));
            }
        }
    }
    spans.push(dim(t, "   "));
    spans.push(label(t, "recovery "));
    let used = Line::from(spans.clone()).width();
    let room = w.saturating_sub(used);
    match f.recovery.last() {
        Some(last) => spans.push(Span::styled(
            fit(
                &format!(
                    "{} stop{}: {last}",
                    f.recovery.len(),
                    if f.recovery.len() == 1 { "" } else { "s" }
                ),
                room,
            ),
            Style::new().fg(t.warn),
        )),
        None => spans.push(dim(t, "nothing needed")),
    }
    frame.render_widget(
        Line::from(spans),
        Rect {
            y: inner.y + 1,
            height: 1,
            ..inner
        },
    );
}

fn lane_order(kind: LaneKind) -> u8 {
    match kind {
        LaneKind::You => 0,
        LaneKind::Lead => 1,
        LaneKind::Reviewer => 2,
        LaneKind::Guard => 3,
        LaneKind::Subagent => 4,
    }
}

/// One row per actor over the job's time; the user's own events are marked
/// with their own glyph.
pub fn timeline(frame: &mut Frame, area: Rect, app: &App, f: &Focus) {
    let t = &app.look.theme;
    let tl = &f.timeline;
    let secs = tl.end / 10;
    let meta = Line::from(dim(t, format!(" 0:00 → {}:{:02} ", secs / 60, secs % 60)));
    let block = panel(t, glyph(app, Icon::Timeline), "timeline", meta, t.border);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 24 || inner.height == 0 {
        return;
    }
    let cell_w = usize::from(inner.width).saturating_sub(LABEL_W + 1);
    let mut lanes: Vec<_> = tl.lanes.iter().collect();
    lanes.sort_by_key(|l| lane_order(l.kind));
    let mut lines = Vec::new();
    for lane in lanes.into_iter().take(usize::from(inner.height)) {
        let color = match lane.kind {
            LaneKind::You => t.title,
            LaneKind::Lead => t.model(Some(&f.who.model)),
            LaneKind::Reviewer => t.reviewer,
            LaneKind::Guard => t.guard,
            LaneKind::Subagent => t.builder,
        };
        let counts = buckets(&lane.marks, 0, tl.end, cell_w);
        let peak = counts.iter().copied().fold(0.0, f64::max);
        let cells: String = counts
            .iter()
            .map(|&n| match lane.kind {
                LaneKind::You | LaneKind::Guard if n > 0.0 => {
                    let icon = if lane.kind == LaneKind::You {
                        Icon::Human
                    } else {
                        Icon::Guard
                    };
                    glyph(app, icon)
                }
                LaneKind::You | LaneKind::Guard => " ",
                _ if n <= 0.0 => " ",
                _ => {
                    const BARS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
                    let i = ((n / peak.max(1.0)) * 8.0).ceil() as usize;
                    BARS[i.clamp(1, 8) - 1]
                }
            })
            .collect();
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:<LABEL_W$}", fit(&lane.label, LABEL_W - 1)),
                Style::new().fg(if lane.kind == LaneKind::You {
                    t.title
                } else {
                    t.label
                }),
            ),
            Span::styled(cells, Style::new().fg(color)),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}
