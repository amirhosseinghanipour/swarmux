//! The Guard / JEV box: decisions settled in code (or by JEV), never by the lead.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_application::{Outcome, Source};

use super::parts::{dim, fit, label};
use crate::app::App;
use crate::theme::{Theme, mix};
use crate::ui::grid::chrome::{self, fingerprint};
use crate::ui::grid::{Panel, PanelId, ViewModel};
use crate::ui::guard::{decision_line, jev_decision_line, jev_line, latency};
use crate::ui::look::Icon;
use crate::ui::widgets::bar_spans;

const NAME_W: usize = 14;
const BAR_W: usize = 12;

pub struct Guard;

impl Panel for Guard {
    fn id(&self) -> PanelId {
        PanelId::Guard
    }
    fn title(&self) -> &'static str {
        "guard"
    }
    fn icon(&self) -> Icon {
        Icon::Guard
    }
    fn min_size(&self) -> (u16, u16) {
        (34, 3)
    }
    fn priority(&self) -> u8 {
        1
    }

    /// The box with as many rows as fit.
    fn render(&self, frame: &mut Frame, area: Rect, vm: &ViewModel, t: &Theme, _tick: u64) {
        let app = vm.app;
        let total = app.guard.stats().total();
        let block = chrome::block(
            self,
            vm,
            t,
            t.guard,
            vec![chrome::title(title(app), t.guard)],
            Some(fingerprint(total)),
        );
        let inner = block.inner(area);
        let room = inner.height as usize;
        let all = rows(vm, t, inner.width as usize);
        let mut keep: Vec<u8> = all.iter().map(|(p, _)| *p).collect();
        keep.sort_unstable();
        keep.truncate(room);
        let lines: Vec<Line> = all
            .into_iter()
            .filter(|(p, _)| keep.contains(p))
            .map(|(_, l)| l)
            .collect();
        frame.render_widget(Paragraph::new(lines).block(block), area);
    }

    /// One line for small windows.
    fn summary(&self, vm: &ViewModel, t: &Theme, _tick: u64) -> Line<'static> {
        let app = vm.app;
        let s = app.guard.stats();
        let mut spans = vec![Span::styled(title(app), Style::new().fg(t.guard))];
        if let Some(share) = s.share(s.allowed) {
            spans.push(label(
                t,
                format!(
                    " allow {:.0}% · ask {} · stop {}",
                    share * 100.0,
                    s.asked,
                    s.stopped
                ),
            ));
        }
        Line::from(spans)
    }
}

/// Rows in display order, with how much each matters when space is short
/// (lower stays longer).
fn rows<'a>(vm: &ViewModel<'a>, t: &Theme, width: usize) -> Vec<(u8, Line<'a>)> {
    let app = vm.app;
    let s = app.guard.stats();
    let mut rows = vec![(2, route_row(vm, t))];

    let tool_gate = match s.share(s.allowed) {
        None => row(
            vm,
            t,
            "tool gate",
            None,
            dim(t, "no tool calls yet"),
            t.guard,
        ),
        Some(share) => {
            let mut mix = format!("allow {:.0}%", share * 100.0);
            for (name, n) in [("ask", s.asked), ("deny", s.denied), ("pass", s.passed)] {
                if n > 0 {
                    mix.push_str(&format!(" · {name} {n}"));
                }
            }
            row(
                vm,
                t,
                "tool gate",
                Some(share),
                Span::styled(
                    format!("{mix} of {}", s.tool_calls()),
                    Style::new().fg(t.guard),
                ),
                t.guard,
            )
        }
    };
    rows.push((0, tool_gate));

    let to_you = s.asked + s.stopped;
    let retry = match (s.total() > 0).then(|| to_you as f64 / s.total() as f64) {
        None => row(vm, t, "retry/stop", None, dim(t, "nothing stopped"), t.warn),
        Some(share) => row(
            vm,
            t,
            "retry/stop",
            Some(share),
            Span::styled(
                format!("stop {} · split → you {}", s.stopped, s.asked),
                Style::new().fg(if s.stopped > 0 { t.error } else { t.warn }),
            ),
            t.warn,
        ),
    };
    rows.push((1, retry));

    let latest = app.guard.recent().next().map_or_else(
        || Line::from(dim(t, "every tool call your jobs make is checked here")),
        |d| match (d.source, d.confidence) {
            (Source::Jev, Some(c)) => jev_decision_line(d, c, t),
            _ => decision_line(d, t),
        },
    );
    rows.push((3, latest));
    rows.push((4, jev_line(app, t)));
    rows.push((5, alarm_line(app, t, width)));
    rows
}

/// How the inbox is doing: new urgent items per ten minutes against the
/// target of one, the priority mix, how long items wait to be noticed and
/// what has been waiting too long. Before anything was raised, what the
/// Guard is for.
fn alarm_line(app: &App, t: &Theme, width: usize) -> Line<'static> {
    let k = app.mission.alarms().kpis(app.now());
    let raised: usize = k.distribution.iter().sum();
    if raised == 0 {
        return Line::from(dim(
            t,
            fit("sharp: runs in code, the lead never sees it", width),
        ));
    }
    let [p1, p2, p3] = k.shares();
    let ttd = k
        .ttd_median
        .map_or("—".to_string(), |ms| format!("{}s", ms / 1000));
    let mut text = format!(
        "alarms {:.1}/10m · P1 {p1:.0}% P2 {p2:.0}% P3 {p3:.0}% · look {ttd}",
        k.urgent_avg_10min
    );
    if k.stale > 0 {
        text.push_str(&format!(" · stale {}", k.stale));
    }
    if k.standing > 0 {
        text.push_str(&format!(" · standing {}", k.standing));
    }
    if k.flood {
        text.push_str(" · FLOOD");
    }
    let warn = k.flood || k.urgent_avg_10min > 1.0;
    Line::from(Span::styled(
        fit(&text, width),
        Style::new().fg(if warn { t.warn } else { t.muted }),
    ))
}

/// `name  ██████▍───── label`; a dim empty track when there is no figure.
fn row(
    vm: &ViewModel,
    t: &Theme,
    name: &str,
    fraction: Option<f64>,
    text: Span<'static>,
    color: Color,
) -> Line<'static> {
    let mut spans = vec![label(t, format!("{name:<NAME_W$}"))];
    match fraction {
        Some(f) => {
            let shown = vm.tween(&format!("guard.{name}"), f);
            spans.extend(bar_spans(
                shown,
                BAR_W,
                &[mix(t.dim, color, 0.55), color],
                t.dim,
            ));
        }
        None => spans.push(dim(t, "─".repeat(BAR_W))),
    }
    spans.push(Span::raw(" "));
    spans.push(text);
    Line::from(spans)
}

/// The latest routing decision: JEV's confidence when it decided, else
/// who picked (Haiku has no confidence to show).
fn route_row(vm: &ViewModel, t: &Theme) -> Line<'static> {
    let app = vm.app;
    let Some(d) = app.routing.recent().find(|d| d.outcome == Outcome::Route) else {
        return row(
            vm,
            t,
            "which session",
            None,
            dim(t, "no routed prompt yet (P)"),
            t.haiku,
        );
    };
    let target = d
        .rule
        .as_deref()
        .and_then(|r| r.strip_prefix("→ "))
        .and_then(|r| r.split([':', ' ']).next())
        .unwrap_or("?")
        .to_string();
    let text = match d.confidence {
        Some(c) => format!("{c:.2} → {target}"),
        None => format!("{} → {target}", d.source.as_str()),
    };
    row(
        vm,
        t,
        "which session",
        d.confidence,
        Span::styled(text, Style::new().fg(t.haiku)),
        t.haiku,
    )
}

/// `GUARD · fork layer · 2,418 decisions · < 0.4 ms`.
fn title(app: &App) -> String {
    let n = app.guard.stats().total();
    let p95 = app
        .guard
        .p95_latency_us()
        .map_or_else(String::new, |us| format!(" · p95 < {}", latency(us)));
    let noun = if n == 1 { "decision" } else { "decisions" };
    format!(" GUARD · fork layer · {} {noun}{p95} ", thousands(n))
}

/// 2418 → "2,418".
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(2418), "2,418");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
