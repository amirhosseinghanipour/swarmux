//! Who sees what: how many tokens each actor read or wrote.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::View;
use super::widgets::{bar, dim, fit, label, model_color, panel, tokens};
use crate::ui::{GREEN, MUTED, short};

const NAME_W: usize = 10;
const TEXT_W: usize = 30;

struct Share {
    name: String,
    color: Color,
    seen: u64,
    text: String,
}

fn shares(view: &View) -> Vec<Share> {
    let Some(job) = view.job else {
        return Vec::new();
    };
    let mut out = vec![Share {
        name: "guard".into(),
        color: GREEN,
        seen: 0,
        text: format!("0 tok · every fork ({})", job.decision_marks.len()),
    }];
    let ctx = if job.context_used == 0 {
        "—".to_string()
    } else {
        short(job.context_used)
    };
    out.push(Share {
        name: "lead".into(),
        color: model_color(Some(&job.model)),
        seen: job.lead_tokens.seen,
        text: format!("{} · splits ({ctx} ctx)", tokens(job.lead_tokens.seen)),
    });
    if let Some(r) = view.review {
        out.push(Share {
            name: "reviewer".into(),
            color: model_color(Some(&r.model)),
            seen: r.lead_tokens.seen,
            text: format!(
                "{} · {} moments",
                tokens(r.lead_tokens.seen),
                r.lead_tokens.turns
            ),
        });
    }
    for s in &job.subagents {
        let rate = s.rate().map_or("—".into(), |r| format!("{r:.0}"));
        out.push(Share {
            name: s.kind.clone(),
            color: model_color(s.model.as_deref()),
            seen: s.tokens.seen,
            text: format!("{} · {rate} t/s", tokens(s.tokens.seen)),
        });
    }
    out
}

pub fn render(frame: &mut Frame, area: Rect, view: &View) {
    let block = panel(" who sees what ".into(), MUTED);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = shares(view);
    if rows.is_empty() {
        frame.render_widget(Paragraph::new(Line::from(dim("no job yet"))), inner);
        return;
    }
    let w = inner.width as usize;
    let bar_w = w.saturating_sub(NAME_W + TEXT_W + 2).max(4);
    let peak = rows.iter().map(|r| r.seen).max().unwrap_or(0).max(1);
    let room = (inner.height as usize).saturating_sub(2);
    let mut lines: Vec<Line> = rows
        .iter()
        .take(room)
        .map(|r| {
            Line::from(vec![
                Span::styled(
                    format!("{:<NAME_W$} ", fit(&r.name, NAME_W)),
                    Style::new().fg(r.color),
                ),
                Span::styled(
                    bar(r.seen as f64 / peak as f64, bar_w),
                    Style::new().fg(r.color),
                ),
                label(format!(" {}", fit(&r.text, TEXT_W))),
            ])
        })
        .collect();
    if rows.len() > room {
        lines.push(dim(format!("+{} more", rows.len() - room)).into());
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        fit("tokens each actor read or wrote, from usage reports", w),
        Style::new().fg(MUTED),
    )));
    frame.render_widget(Paragraph::new(lines), inner);
}
