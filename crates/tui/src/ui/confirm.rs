//! The confirmation of an irreversible call: the user names the target after
//! the prompt has armed. The list of choices rotates and the dialog shifts
//! from call to call, so a habitual keystroke never lands on the answer.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

use crate::app::App;
use crate::inbox::Confirm;

/// Rows the dialog takes, borders included.
pub const HEIGHT: u16 = 6;

pub fn render(frame: &mut Frame, area: Rect, app: &App, c: &Confirm) {
    let t = &app.look.theme;
    let now = app.now();
    let ch = &c.challenge;
    let pad = " ".repeat(usize::from(ch.shift()) * 2);
    let bold = |color| Style::new().fg(color).add_modifier(Modifier::BOLD);
    let width = usize::from(area.width).saturating_sub(4);
    let cut = |s: &str, room: usize| -> String {
        if s.chars().count() > room {
            let head: String = s.chars().take(room.saturating_sub(1)).collect();
            format!("{head}…")
        } else {
            s.to_string()
        }
    };

    let mut lines = vec![Line::from(vec![
        Span::raw(pad.clone()),
        Span::styled("R4 ", bold(t.error)),
        Span::raw(cut(&ch.command, width.saturating_sub(pad.len() + 3))),
    ])];
    lines.push(Line::from(vec![
        Span::raw(pad.clone()),
        Span::styled("this cannot be undone", Style::new().fg(t.muted)),
    ]));
    if !ch.armed(now) {
        let left = ch.arming_left(now);
        lines.push(Line::from(vec![
            Span::raw(pad.clone()),
            Span::styled(
                format!("arming… {}.{}s", left / 1000, left % 1000 / 100),
                Style::new().fg(t.warn),
            ),
        ]));
    } else if c.pick {
        let mut spans = vec![
            Span::raw(pad.clone()),
            Span::styled("pick the target: ", Style::new().fg(t.label)),
        ];
        for (i, option) in ch.options().iter().enumerate() {
            spans.push(Span::styled(format!("{}", i + 1), bold(t.title)));
            spans.push(Span::raw(format!(" {}   ", cut(option, 24))));
        }
        lines.push(Line::from(spans));
    } else {
        let (before, at, after) = c.input.split_at_cursor();
        lines.push(Line::from(vec![
            Span::raw(pad.clone()),
            Span::styled("type the target: ", Style::new().fg(t.label)),
            Span::raw(before),
            Span::styled(at, Style::new().add_modifier(Modifier::REVERSED)),
            Span::raw(after),
        ]));
    }
    let hint = if c.wrong {
        Span::styled("not the target · try again", bold(t.error))
    } else if ch.armed(now) {
        Span::styled(
            "⏎ confirm · tab type or pick · esc cancel",
            Style::new().fg(t.muted),
        )
    } else {
        Span::styled(
            "keys count once it has armed · esc cancel",
            Style::new().fg(t.muted),
        )
    };
    lines.push(Line::from(vec![Span::raw(pad), hint]));

    let block = super::rounded(t)
        .border_style(Style::new().fg(t.error))
        .title(Span::styled(" irreversible · confirm ", bold(t.error)));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}
