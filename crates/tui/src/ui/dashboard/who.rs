//! Who sees what: how many tokens each actor read or wrote, and the same
//! split by model family as a ring.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_domain::Family;

use super::View;
use super::parts::{dim, fit, label, tokens};
use crate::theme::{Theme, mix};
use crate::ui::grid::chrome::{self, fingerprint};
use crate::ui::grid::{Panel, PanelId, ViewModel};
use crate::ui::look::Icon;
use crate::ui::short;
use crate::ui::widgets::{Ring, bar_spans};

const NAME_W: usize = 10;
const TEXT_W: usize = 30;

struct Share {
    name: String,
    model: Option<String>,
    color: Color,
    seen: u64,
    text: String,
}

fn shares(view: &View, t: &Theme) -> Vec<Share> {
    let Some(job) = view.job else {
        return Vec::new();
    };
    let mut out = vec![Share {
        name: "guard".into(),
        model: None,
        color: t.guard,
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
        model: Some(job.model.clone()),
        color: t.model(Some(&job.model)),
        seen: job.lead_tokens.seen,
        text: format!("{} · splits ({ctx} ctx)", tokens(job.lead_tokens.seen)),
    });
    if let Some(r) = view.review {
        out.push(Share {
            name: "reviewer".into(),
            model: Some(r.model.clone()),
            color: t.model(Some(&r.model)),
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
            model: s.model.clone(),
            color: t.model(s.model.as_deref()),
            seen: s.tokens.seen,
            text: format!("{} · {rate} t/s", tokens(s.tokens.seen)),
        });
    }
    out
}

pub struct Who;

impl Panel for Who {
    fn id(&self) -> PanelId {
        PanelId::Who
    }
    fn title(&self) -> &'static str {
        "who sees what"
    }
    fn icon(&self) -> Icon {
        Icon::Who
    }
    fn min_size(&self) -> (u16, u16) {
        (30, 4)
    }
    fn priority(&self) -> u8 {
        7
    }
    fn render(&self, frame: &mut Frame, area: Rect, vm: &ViewModel, t: &Theme, _tick: u64) {
        let rows = shares(&vm.view, t);
        let total: u64 = rows.iter().map(|r| r.seen).sum();
        let block = chrome::block(
            self,
            vm,
            t,
            t.border,
            vec![chrome::title(" who sees what ", t.muted)],
            Some(fingerprint(total / 1000)),
        );
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if rows.is_empty() {
            frame.render_widget(Paragraph::new(Line::from(dim(t, "no job yet"))), inner);
            return;
        }
        let w = inner.width as usize;
        let text_w = TEXT_W.min(w / 3);
        let bar_w = w.saturating_sub(NAME_W + text_w + 2).max(4);
        let peak = rows.iter().map(|r| r.seen).max().unwrap_or(0).max(1);
        let room = (inner.height as usize).saturating_sub(2);
        let mut lines: Vec<Line> = rows
            .iter()
            .take(room)
            .map(|r| {
                let share = vm.tween(&format!("who.{}", r.name), r.seen as f64 / peak as f64);
                let mut spans = vec![Span::styled(
                    format!("{:<NAME_W$} ", fit(&r.name, NAME_W)),
                    Style::new().fg(r.color),
                )];
                spans.extend(bar_spans(
                    share,
                    bar_w,
                    &[mix(t.dim, r.color, 0.45), r.color],
                    t.dim,
                ));
                spans.push(label(t, format!(" {}", fit(&r.text, text_w))));
                Line::from(spans)
            })
            .collect();
        if rows.len() > room {
            lines.push(dim(t, format!("+{} more", rows.len() - room)).into());
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            fit("tokens each actor read or wrote, from usage reports", w),
            Style::new().fg(t.muted),
        )));
        frame.render_widget(Paragraph::new(lines), inner);
    }
    fn summary(&self, vm: &ViewModel, t: &Theme, _tick: u64) -> Line<'static> {
        let rows = shares(&vm.view, t);
        if rows.is_empty() {
            return Line::from(dim(t, " who sees what · no job yet"));
        }
        let mut spans = vec![label(t, " who sees what ")];
        for r in rows.iter().filter(|r| r.seen > 0) {
            spans.push(Span::styled(
                format!("{} {} ", r.name, tokens(r.seen)),
                Style::new().fg(r.color),
            ));
        }
        Line::from(spans)
    }
}

/// Tokens by model family: (family, tokens, colour), largest first.
fn by_family(view: &View, t: &Theme) -> Vec<(&'static str, f64, Color)> {
    let mut out: Vec<(&'static str, f64, Color)> = Vec::new();
    for s in shares(view, t) {
        let Some(model) = s.model.as_deref() else {
            continue;
        };
        if s.seen == 0 {
            continue;
        }
        let family = Family::of(model).as_str();
        match out.iter_mut().find(|(f, _, _)| *f == family) {
            Some(entry) => entry.1 += s.seen as f64,
            None => out.push((family, s.seen as f64, t.model(Some(model)))),
        }
    }
    // Families that share a colour (opus and sonnet in some themes) are
    // told apart by a shade.
    for i in 1..out.len() {
        if out[..i].iter().any(|(_, _, c)| *c == out[i].2) {
            out[i].2 = mix(out[i].2, t.title, 0.35);
        }
    }
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

/// The model mix as a braille ring.
pub struct Mix;

impl Panel for Mix {
    fn id(&self) -> PanelId {
        PanelId::Mix
    }
    fn title(&self) -> &'static str {
        "model mix"
    }
    fn icon(&self) -> Icon {
        Icon::Mix
    }
    fn min_size(&self) -> (u16, u16) {
        (20, 5)
    }
    fn priority(&self) -> u8 {
        12
    }
    fn render(&self, frame: &mut Frame, area: Rect, vm: &ViewModel, t: &Theme, _tick: u64) {
        let mix = by_family(&vm.view, t);
        let block = chrome::block(
            self,
            vm,
            t,
            t.border,
            vec![chrome::title(" model mix ", t.muted)],
            None,
        );
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if mix.is_empty() {
            frame.render_widget(
                Paragraph::new(Line::from(dim(t, "tokens by model show up here"))),
                inner,
            );
            return;
        }
        let total: f64 = mix.iter().map(|m| m.1).sum();
        let center = short(total as u64);
        frame.render_widget(
            Ring::new(&mix)
                .center(&center)
                .legend(true)
                .track(t.dim)
                .text(t.text),
            inner,
        );
    }
    fn summary(&self, vm: &ViewModel, t: &Theme, _tick: u64) -> Line<'static> {
        let mix = by_family(&vm.view, t);
        let total: f64 = mix.iter().map(|m| m.1).sum();
        if total <= 0.0 {
            return Line::from(dim(t, " model mix · —"));
        }
        let mut spans = vec![label(t, " mix ")];
        for (name, n, color) in mix {
            spans.push(Span::styled(
                format!("{name} {:.0}% ", n / total * 100.0),
                Style::new().fg(color),
            ));
        }
        Line::from(spans)
    }
}
