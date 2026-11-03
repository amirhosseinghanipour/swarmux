//! The attention inbox: the only panel with a warm border, a breathing one
//! while something is P1.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_domain::{AttentionItem, AttentionKind, Millis, Priority};

use super::{age, glyph, panel, sigil};
use crate::app::App;
use crate::theme::{Theme, mix};
use crate::ui::dashboard::{dim, fit};
use crate::ui::look::Icon;

/// Lines an item takes.
const ITEM_ROWS: u16 = 2;
/// How long a fresh item shows its arrival highlight, in milliseconds.
const ARRIVAL_MS: Millis = 600;
/// How much darker the breathing border gets at its dimmest.
const BREATH_DEPTH: f64 = 0.17;

pub fn render(frame: &mut Frame, area: Rect, app: &App, now: Millis) {
    let t = &app.look.theme;
    let m = &app.mission;
    let count = |p: Priority| m.items.iter().filter(|i| i.priority == p).count();
    let meta = if m.items.is_empty() {
        Line::from(Span::styled(" clear ", Style::new().fg(t.ok)))
    } else {
        Line::from(vec![
            Span::styled(
                format!(" P1 {}", count(Priority::P1)),
                prio_style(t, Priority::P1),
            ),
            Span::styled(
                format!(" · P2 {}", count(Priority::P2)),
                prio_style(t, Priority::P2),
            ),
            Span::styled(
                format!(" · P3 {} ", count(Priority::P3)),
                prio_style(t, Priority::P3),
            ),
        ])
    };
    let border = border_color(app, count(Priority::P1) > 0);
    let block = panel(t, glyph(app, Icon::Inbox), "needs you", meta, border);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 12 || inner.height < ITEM_ROWS {
        return;
    }

    if m.items.is_empty() {
        let hint = if app.sessions.is_empty() {
            "press 6, then n, to add a session"
        } else {
            "all quiet · nothing needs you"
        };
        frame.render_widget(
            Paragraph::new(Line::from(dim(t, fit(hint, usize::from(inner.width))))),
            Rect { height: 1, ..inner },
        );
        return;
    }

    let capacity = usize::from(inner.height / ITEM_ROWS).max(1);
    let selected = m.selected_index().unwrap_or(0);
    let start = (selected + 1).saturating_sub(capacity);
    let mut lines = Vec::new();
    for (i, item) in m.items.iter().enumerate().skip(start).take(capacity) {
        let [a, b] = item_lines(app, item, (i, selected), usize::from(inner.width), now);
        lines.push(a);
        lines.push(b);
    }
    frame.render_widget(Paragraph::new(lines), inner);
    let hidden = m.items.len().saturating_sub(start + capacity);
    if hidden > 0 {
        let more = format!(" ↓ {hidden} more ");
        let w = more.chars().count() as u16;
        if inner.width > w {
            frame.render_widget(
                Line::from(dim(t, more)),
                Rect::new(inner.right() - w, area.bottom() - 1, w, 1),
            );
        }
    }
}

/// The inbox border: the accent, breathing when something is P1.
fn border_color(app: &App, urgent: bool) -> Color {
    let t = &app.look.theme;
    if !(urgent && app.look.animated()) {
        return t.border_focus;
    }
    let seconds = app.frame as f64 / f64::from(app.look.fps());
    // 0.5 Hz: one breath every two seconds.
    let breath = 0.5 - 0.5 * (std::f64::consts::TAU * 0.5 * seconds).cos();
    mix(t.border_focus, t.panel_bg, BREATH_DEPTH * (1.0 - breath))
}

fn prio_style(t: &Theme, p: Priority) -> Style {
    match p {
        Priority::P1 => Style::new().fg(t.error).add_modifier(Modifier::BOLD),
        Priority::P2 => Style::new().fg(t.warn),
        Priority::P3 => Style::new().fg(t.muted),
    }
}

fn kind_icon(kind: AttentionKind) -> Icon {
    match kind {
        AttentionKind::Approval => Icon::Approval,
        AttentionKind::Question => Icon::Question,
        AttentionKind::JobFailed => Icon::Failed,
        AttentionKind::ReviewConcerns => Icon::Concerns,
        AttentionKind::ResultReady => Icon::Result,
        AttentionKind::Stalled => Icon::Stalled,
        AttentionKind::GateEscalated => Icon::Guard,
    }
}

/// `m:ss` for a countdown.
fn countdown(ms: Millis) -> String {
    let s = ms.max(0) / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The two lines of one item. The selected item shows its keys instead of
/// its detail.
pub(super) fn item_lines(
    app: &App,
    item: &AttentionItem,
    (index, selected): (usize, usize),
    width: usize,
    now: Millis,
) -> [Line<'static>; 2] {
    let t = &app.look.theme;
    let selected = index == selected;
    let session = item
        .session
        .and_then(|id| app.sessions.iter().find(|s| s.id == id));
    let (name, model) =
        session.map_or(("—", None), |s| (s.name.as_str(), Some(s.model.as_str())));

    // Fresh items flash once; text being read is never tweened.
    let fresh = (now - item.since).clamp(0, ARRIVAL_MS);
    // Only the top item moves; everything else is still between updates.
    let arrival =
        if app.look.animated() && index == 0 && item.priority != Priority::P3 && fresh < ARRIVAL_MS
        {
            1.0 - fresh as f64 / ARRIVAL_MS as f64
        } else {
            0.0
        };
    let title_color = mix(t.text, t.title, arrival);

    let marker = if selected {
        Span::styled(
            glyph(app, Icon::Lead),
            Style::new().fg(t.border_focus).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::raw(" ")
    };
    let when = age(now - item.since);
    let head = vec![
        marker,
        Span::styled(
            format!(" {} ", item.priority.as_str()),
            prio_style(t, item.priority),
        ),
        Span::styled(
            glyph(app, kind_icon(item.kind)),
            prio_style(t, item.priority),
        ),
        Span::styled(
            format!(" {:<8} ", item.kind.as_str()),
            Style::new().fg(t.label),
        ),
        Span::styled(
            sigil(name).to_string(),
            Style::new().fg(t.model(model)).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {} ", fit(name, 12)), Style::new().fg(t.muted)),
    ];
    let used = Line::from(head.clone()).width() + when.chars().count() + 2;
    let title = fit(&item.title, width.saturating_sub(used));
    let mut first = head;
    let pad = width.saturating_sub(
        Line::from(first.clone()).width() + title.chars().count() + when.chars().count(),
    );
    first.push(Span::styled(
        title,
        Style::new().fg(title_color).add_modifier(if selected {
            Modifier::BOLD
        } else {
            Modifier::empty()
        }),
    ));
    first.push(Span::raw(" ".repeat(pad)));
    first.push(Span::styled(when, Style::new().fg(t.muted)));

    let indent = Span::raw("   ");
    let second = if selected {
        let mut spans = vec![indent];
        let key = |k: &str| {
            Span::styled(
                k.to_string(),
                Style::new().fg(t.title).add_modifier(Modifier::BOLD),
            )
        };
        let txt = |s: &str| Span::styled(s.to_string(), Style::new().fg(t.muted));
        if item.kind == AttentionKind::Approval {
            spans.extend([
                key("y"),
                txt(" approve · "),
                key("n"),
                txt(" deny · "),
                key("a"),
                txt(" always · "),
            ]);
        } else {
            spans.extend([key("x"), txt(" dismiss · ")]);
        }
        spans.extend([key("⏎"), txt(" open")]);
        if let Some(deadline) = item.deadline {
            spans.push(txt(&format!(" · {} left", countdown(deadline - now))));
        }
        // The grade goes last: it is dropped before a key is cut.
        let grade = format!(" · {}", item.likelihood.as_str());
        if Line::from(spans.clone()).width() + grade.chars().count() <= width {
            spans.push(txt(&grade));
        }
        Line::from(spans)
    } else {
        let text = format!("{} · {}", item.likelihood.as_str(), item.detail);
        Line::from(vec![indent, dim(t, fit(&text, width.saturating_sub(3)))])
    };
    [Line::from(first), second]
}
