//! Small pieces the Work screen's panes share: headings with a rule,
//! wrapping, side by side columns, marks that follow the icon set.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use swarmux_application::console::PriorityKind;

use crate::app::App;
use crate::theme::Theme;
use crate::ui::look::IconSet;

pub(super) type Lines = Vec<Line<'static>>;

/// Marks that read the same in every font: geometric shapes, or ASCII.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mark {
    /// Something waits for the user.
    Ask,
    /// Failed or escalated.
    Bad,
    /// Verified: a filled square.
    Ready,
    /// Working or queued: an empty square.
    Open,
    /// A check passed, or landed.
    Pass,
    /// A check failed.
    Fail,
    /// Reported, not blocking.
    Flag,
    /// Did not run.
    Idle,
}

pub(super) fn mark(app: &App, m: Mark) -> &'static str {
    let ascii = app.look.icons == IconSet::Ascii;
    match (m, ascii) {
        (Mark::Ask, false) => "◆",
        (Mark::Ask, true) => "*",
        (Mark::Bad, false) => "×",
        (Mark::Bad, true) => "x",
        (Mark::Ready, false) => "■",
        (Mark::Ready, true) => "#",
        (Mark::Open, false) => "□",
        (Mark::Open, true) => "o",
        (Mark::Pass, false) => "✓",
        (Mark::Pass, true) => "v",
        (Mark::Fail, false) => "✗",
        (Mark::Fail, true) => "x",
        (Mark::Flag, false) => "◆",
        (Mark::Flag, true) => "!",
        (Mark::Idle, false) => "·",
        (Mark::Idle, true) => ".",
    }
}

/// The rule character, and the ellipsis and arrow that follow the set.
pub(super) fn rule(app: &App) -> char {
    if app.look.icons == IconSet::Ascii {
        '-'
    } else {
        '─'
    }
}

pub(super) fn arrow(app: &App) -> &'static str {
    if app.look.icons == IconSet::Ascii {
        "->"
    } else {
        "→"
    }
}

/// The colour of a priority.
pub(super) fn priority_color(t: &Theme, p: PriorityKind) -> ratatui::style::Color {
    match p {
        PriorityKind::P1 => t.error,
        PriorityKind::P2 => t.warn,
        PriorityKind::P3 => t.muted,
    }
}

pub(super) fn priority_text(p: PriorityKind) -> &'static str {
    match p {
        PriorityKind::P1 => "P1",
        PriorityKind::P2 => "P2",
        PriorityKind::P3 => "P3",
    }
}

/// `cut` to `max` cells, with `…` where it was cut.
pub(super) fn fit(s: &str, max: usize) -> String {
    let cells = |s: &str| Line::from(s.to_string()).width();
    if cells(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars() {
        if cells(&out) + cells(&c.to_string()) + 1 > max {
            break;
        }
        out.push(c);
    }
    if max > 0 {
        out.push('…');
    }
    out
}

/// A heading: `TITLE  note ─────────`, the rule filling the width.
pub(super) fn heading(
    app: &App,
    title: &str,
    note: &str,
    width: usize,
    color: ratatui::style::Color,
) -> Line<'static> {
    let t = &app.look.theme;
    let mut spans = vec![Span::styled(
        title.to_string(),
        Style::new().fg(color).add_modifier(Modifier::BOLD),
    )];
    if !note.is_empty() {
        spans.push(Span::styled(format!("  {note}"), Style::new().fg(t.muted)));
    }
    let used = Line::from(spans.clone()).width();
    if used + 2 <= width {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            rule(app).to_string().repeat(width - used - 1),
            Style::new().fg(t.dim),
        ));
    }
    Line::from(spans)
}

/// `key  value` with the key in the label colour, padded to `key_w`.
pub(super) fn pair(t: &Theme, key: &str, key_w: usize, value: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!("{key:<key_w$} "),
        Style::new().fg(t.muted),
    )];
    spans.extend(value);
    Line::from(spans)
}

/// Splits `text` into lines of at most `width` cells at spaces; a word
/// longer than the line is cut.
pub(super) fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out: Vec<String> = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let word = if Line::from(word.to_string()).width() > width {
                fit(word, width)
            } else {
                word.to_string()
            };
            let need = Line::from(line.clone()).width()
                + usize::from(!line.is_empty())
                + Line::from(word.clone()).width();
            if need > width && !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&word);
        }
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// `text` quoted, wrapped, each line indented by `indent`; at most `max`
/// lines, the last ending in `…` when more was cut.
pub(super) fn quote(text: &str, width: usize, indent: usize, max: usize, style: Style) -> Lines {
    let inner = width.saturating_sub(indent + 2).max(8);
    let mut lines = wrap_text(text.trim(), inner);
    let cut = lines.len() > max;
    lines.truncate(max);
    if cut && let Some(last) = lines.last_mut() {
        *last = format!("{}…", fit(last, inner.saturating_sub(1)));
    }
    let n = lines.len();
    lines
        .into_iter()
        .enumerate()
        .map(|(i, l)| {
            let open = if i == 0 { "“" } else { " " };
            let close = if i + 1 == n { "”" } else { "" };
            Line::from(vec![
                Span::raw(" ".repeat(indent)),
                Span::styled(format!("{open}{l}{close}"), style),
            ])
        })
        .collect()
}

/// Two columns side by side: `left` padded to `left_w`, a gap, `right`.
pub(super) fn columns(left: Lines, left_w: usize, gap: usize, right: Lines) -> Lines {
    let rows = left.len().max(right.len());
    let mut left = left.into_iter();
    let mut right = right.into_iter();
    (0..rows)
        .map(|_| {
            let mut spans: Vec<Span<'static>> = Vec::new();
            let l = left.next().unwrap_or_default();
            let pad = left_w.saturating_sub(l.width());
            spans.extend(l.spans);
            spans.push(Span::raw(" ".repeat(pad + gap)));
            if let Some(r) = right.next() {
                spans.extend(r.spans);
            }
            Line::from(spans)
        })
        .collect()
}

/// `38s`, `2m02s`, `1h05m`.
pub(super) fn duration(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0 if ms > 0 => format!("{ms}ms"),
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m{:02}s", s / 60, s % 60),
        _ => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
    }
}

/// `$9.24`.
pub(super) fn usd(x: f64) -> String {
    format!("${x:.2}")
}

/// Spans of `text` in `style`.
pub(super) fn span(text: impl Into<String>, style: Style) -> Span<'static> {
    Span::styled(text.into(), style)
}

/// Where an outcome is in its life, as a word.
pub(super) fn lifecycle_word(l: swarmux_application::console::Lifecycle) -> &'static str {
    use swarmux_application::console::Lifecycle as L;
    match l {
        L::Draft => "draft",
        L::Linted => "linted",
        L::Queued => "queued",
        L::Running => "running",
        L::Verifying => "verifying",
        L::Delivered => "delivered",
        L::Escalated => "escalated",
        L::Refused => "refused",
        L::Done => "done",
        L::Failed => "failed",
        L::Interrupted => "interrupted",
        L::Cancelled => "cancelled",
    }
}

/// `14:21:46`: the time of day of `ms`, UTC, as the ledger writes it.
pub(super) fn clock(ms: swarmux_domain::Millis) -> String {
    let s = ms.rem_euclid(86_400_000) / 1000;
    format!("{:02}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
}

/// The vertical rule between the queue and the outcome.
pub(super) fn rule_column(app: &App) -> Span<'static> {
    let c = if app.look.icons == IconSet::Ascii {
        "|"
    } else {
        "│"
    };
    Span::styled(c, Style::new().fg(app.look.theme.dim))
}
