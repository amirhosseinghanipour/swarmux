//! The action stream: the job's tool calls as a tree, results under their
//! calls, subagents indented, diff lines tinted, and the "you were here"
//! line where the user last looked.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::parts::diff_style;
use crate::app::App;
use crate::focus::Focus;
use crate::focus::stream::{Row, RowKind};
use crate::theme::Theme;
use crate::ui::dashboard::{dim, fit};
use crate::ui::look::{Icon, IconSet};
use crate::ui::mission::{glyph, panel};

/// Width of the time column.
const TIME_W: usize = 6;

pub fn render(frame: &mut Frame, area: Rect, app: &App, f: &Focus) {
    let t = &app.look.theme;
    let total = f.rows.len() + usize::from(f.here.is_some());
    let block_meta = |scroll: usize| {
        Line::from(dim(
            t,
            if scroll == 0 {
                format!(" {} rows · following ", f.rows.len())
            } else {
                format!(" {} rows · {scroll} back · end follows ", f.rows.len())
            },
        ))
    };
    let state = &app.focus_state;
    // The block needs the scroll after clamping, so measure first.
    let height = usize::from(area.height.saturating_sub(2));
    let max_scroll = total.saturating_sub(height);
    state.max_scroll.set(max_scroll);
    let scroll = state.scroll.min(max_scroll);
    let block = panel(
        t,
        glyph(app, Icon::Log),
        "action stream",
        block_meta(scroll),
        t.border,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 16 || inner.height == 0 {
        return;
    }
    if f.rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(dim(t, "waiting for the first action"))),
            Rect { height: 1, ..inner },
        );
        return;
    }
    let end = total - scroll;
    let start = end.saturating_sub(height);
    let w = usize::from(inner.width);
    let lines: Vec<Line> = (start..end)
        .map(|i| match f.here {
            Some(h) if i == h => marker(app, w, f.rows.len().saturating_sub(h)),
            Some(h) if i > h => row_line(app, f, &f.rows[i - 1], w),
            _ => row_line(app, f, &f.rows[i], w),
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// `▶ you were here ──────── 4 new`.
fn marker(app: &App, width: usize, new: usize) -> Line<'static> {
    let t = &app.look.theme;
    let rule = if app.look.icons == IconSet::Ascii {
        '-'
    } else {
        '─'
    };
    let head = format!("{} you were here ", glyph(app, Icon::Here));
    let tail = format!(" {new} new");
    let fill = width.saturating_sub(head.chars().count() + tail.chars().count());
    Line::from(vec![
        Span::styled(head, Style::new().fg(t.title).add_modifier(Modifier::BOLD)),
        Span::styled(rule.to_string().repeat(fill), Style::new().fg(t.dim)),
        Span::styled(tail, Style::new().fg(t.title)),
    ])
}

fn tree(app: &App, last: bool) -> &'static str {
    match (app.look.icons == IconSet::Ascii, last) {
        (true, true) => "`-",
        (true, false) => "|-",
        (false, true) => "└─",
        (false, false) => "├─",
    }
}

fn row_line(app: &App, f: &Focus, row: &Row, width: usize) -> Line<'static> {
    let t = &app.look.theme;
    let time = Span::styled(format!("{:>TIME_W$} ", row.time), Style::new().fg(t.dim));
    let indent = if row.depth > 0 {
        let bar = if app.look.icons == IconSet::Ascii {
            "| "
        } else {
            "│ "
        };
        Span::styled(bar, Style::new().fg(t.dim))
    } else {
        Span::raw("")
    };
    let room = width.saturating_sub(TIME_W + 1 + indent.width());
    let mut spans = vec![time, indent];
    match &row.kind {
        RowKind::Call { tool, subject } => {
            let hue = actor_hue(t, f, row);
            let who = if row.depth > 0 {
                format!("{} ", row.actor)
            } else {
                String::new()
            };
            spans.push(Span::styled(
                format!("{} ", glyph(app, Icon::Call)),
                Style::new().fg(hue),
            ));
            spans.push(Span::styled(who, Style::new().fg(hue)));
            spans.push(Span::styled(
                format!("{tool} "),
                Style::new().fg(t.title).add_modifier(Modifier::BOLD),
            ));
            let used: usize = spans.iter().map(Span::width).sum::<usize>() - TIME_W - 1;
            spans.push(Span::styled(
                fit(subject, room.saturating_sub(used)),
                Style::new().fg(t.text),
            ));
        }
        RowKind::Result { ok } => {
            // The time column stays empty under a call.
            spans[0] = Span::raw(" ".repeat(TIME_W + 1));
            let (icon, color) = if *ok {
                (Icon::Result, t.ok)
            } else {
                (Icon::Failed, t.error)
            };
            spans.push(Span::styled(
                format!("{} ", tree(app, row.last)),
                Style::new().fg(t.dim),
            ));
            spans.push(Span::styled(
                format!("{} ", glyph(app, icon)),
                Style::new().fg(color),
            ));
            let used: usize = spans.iter().map(Span::width).sum::<usize>() - TIME_W - 1;
            spans.push(Span::styled(
                fit(&row.text, room.saturating_sub(used)),
                Style::new().fg(if *ok { t.muted } else { t.error }),
            ));
        }
        RowKind::Say => {
            spans.push(Span::styled("» ", Style::new().fg(t.label)));
            spans.push(Span::styled(
                fit(&row.text, room.saturating_sub(2)),
                Style::new().fg(t.text),
            ));
        }
        RowKind::Diff(kind) => {
            let style = diff_style(t, *kind);
            let text = fit(&row.text, room);
            let pad = room.saturating_sub(text.chars().count());
            spans.push(Span::styled(format!("{text}{}", " ".repeat(pad)), style));
        }
        RowKind::Milestone => {
            spans.push(Span::styled(
                format!("{} ", glyph(app, Icon::StateRun)),
                Style::new().fg(t.label),
            ));
            spans.push(Span::styled(
                fit(&row.text, room.saturating_sub(2)),
                Style::new().fg(t.muted).add_modifier(Modifier::BOLD),
            ));
        }
        RowKind::Note => spans.push(Span::styled(fit(&row.text, room), Style::new().fg(t.dim))),
    }
    Line::from(spans)
}

/// The colour of whoever did it: the lead's model, or the subagent's.
fn actor_hue(t: &Theme, f: &Focus, row: &Row) -> ratatui::style::Color {
    if row.depth == 0 {
        return t.model(Some(&f.who.model));
    }
    f.agents
        .iter()
        .find(|a| a.kind == row.actor)
        .map_or(t.muted, |a| t.model(a.model.as_deref()))
}
