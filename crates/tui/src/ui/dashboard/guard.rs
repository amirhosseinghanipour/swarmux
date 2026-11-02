//! The Guard / JEV box: decisions settled in code (or by JEV), never by the lead.

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_application::{Outcome, Source};

use super::widgets::{bar, dim, fit, label, panel};
use crate::app::App;
use crate::ui::guard::{decision_line, jev_decision_line, jev_line, latency};
use crate::ui::{BLUE, DIM, GREEN, ORANGE, RED};

const NAME_W: usize = 14;
const BAR_W: usize = 12;

/// Rows in display order, with how much each matters when space is short
/// (lower stays longer).
fn rows(app: &App, width: usize) -> Vec<(u8, Line<'_>)> {
    let s = app.guard.stats();
    let mut rows = vec![(2, route_row(app))];

    let tool_gate = match s.share(s.allowed) {
        None => row("tool gate", None, dim("no tool calls yet"), GREEN),
        Some(share) => {
            let mut mix = format!("allow {:.0}%", share * 100.0);
            for (name, n) in [("ask", s.asked), ("deny", s.denied), ("pass", s.passed)] {
                if n > 0 {
                    mix.push_str(&format!(" · {name} {n}"));
                }
            }
            row(
                "tool gate",
                Some(share),
                Span::styled(
                    format!("{mix} of {}", s.tool_calls()),
                    Style::new().fg(GREEN),
                ),
                GREEN,
            )
        }
    };
    rows.push((0, tool_gate));

    let to_you = s.asked + s.stopped;
    let retry = match (s.total() > 0).then(|| to_you as f64 / s.total() as f64) {
        None => row("retry/stop", None, dim("nothing stopped"), ORANGE),
        Some(share) => row(
            "retry/stop",
            Some(share),
            Span::styled(
                format!("stop {} · split → you {}", s.stopped, s.asked),
                Style::new().fg(if s.stopped > 0 { RED } else { ORANGE }),
            ),
            ORANGE,
        ),
    };
    rows.push((1, retry));

    let latest = app.guard.recent().next().map_or_else(
        || Line::from(dim("every tool call your jobs make is checked here")),
        |d| match (d.source, d.confidence) {
            (Source::Jev, Some(c)) => jev_decision_line(d, c),
            _ => decision_line(d),
        },
    );
    rows.push((3, latest));
    rows.push((4, jev_line(app)));
    rows.push((
        5,
        Line::from(Span::styled(
            fit("sharp: runs in code, the lead never sees it", width),
            Style::new().fg(DIM),
        )),
    ));
    rows
}

/// `name  ██████░░░░░░ label`; a dim empty track when there is no figure.
fn row(
    name: &str,
    fraction: Option<f64>,
    text: Span<'static>,
    color: ratatui::style::Color,
) -> Line<'static> {
    let track = match fraction {
        Some(f) => Span::styled(bar(f, BAR_W), Style::new().fg(color)),
        None => dim("─".repeat(BAR_W)),
    };
    Line::from(vec![
        label(format!("{name:<NAME_W$}")),
        track,
        Span::raw(" "),
        text,
    ])
}

/// The latest routing decision: JEV's confidence when it decided, else
/// who picked (Haiku has no confidence to show).
fn route_row(app: &App) -> Line<'static> {
    let Some(d) = app.routing.recent().find(|d| d.outcome == Outcome::Route) else {
        return row("which session", None, dim("no routed prompt yet (P)"), BLUE);
    };
    let target = d
        .rule
        .as_deref()
        .and_then(|r| r.strip_prefix("→ "))
        .and_then(|r| r.split([':', ' ']).next())
        .unwrap_or("?")
        .to_string();
    match d.confidence {
        Some(c) => row(
            "which session",
            Some(c),
            Span::styled(format!("{c:.2} → {target}"), Style::new().fg(BLUE)),
            BLUE,
        ),
        None => row(
            "which session",
            None,
            Span::styled(
                format!("{} → {target}", d.source.as_str()),
                Style::new().fg(BLUE),
            ),
            BLUE,
        ),
    }
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

/// The box with as many rows as fit.
pub fn panel_box(frame: &mut Frame, area: Rect, app: &App) {
    let block = panel(title(app), GREEN);
    let inner = block.inner(area);
    let room = inner.height as usize;
    let all = rows(app, inner.width as usize);
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
pub fn one_line(app: &App) -> Line<'static> {
    let s = app.guard.stats();
    let mut spans = vec![Span::styled(title(app), Style::new().fg(GREEN))];
    if let Some(share) = s.share(s.allowed) {
        spans.push(label(format!(
            " allow {:.0}% · ask {} · stop {}",
            share * 100.0,
            s.asked,
            s.stopped
        )));
    }
    Line::from(spans)
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
