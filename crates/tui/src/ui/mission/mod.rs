//! Mission control, the home screen: does anything need me, who is doing
//! what, will I hit a limit. A top strip, the attention inbox, the session
//! wall and (on tall windows) the change log.

mod changes;
mod extra;
mod inbox;
pub mod layout;
mod peek;
mod strip;
mod wall;

#[cfg(test)]
mod alarm_tests;
#[cfg(test)]
pub(crate) mod tests;

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph},
};

use self::layout::Panel;
use crate::app::App;
use crate::theme::Theme;
use crate::ui::dashboard::{dim, label};
use crate::ui::look::Icon;

/// Smallest window Mission control lays out in.
const MIN_SIZE: (u16, u16) = (60, 12);
/// From this body width the strip fits its gauges side by side.
pub(crate) const WIDE: u16 = 100;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    if area.width < MIN_SIZE.0 || area.height < MIN_SIZE.1 {
        let lines = vec![
            Line::from(label(t, "make the window bigger")).centered(),
            Line::from(dim(t, "mission control needs 80×24")).centered(),
        ];
        frame.render_widget(Paragraph::new(lines), area);
        return;
    }
    let now = app.now();
    let screen = frame.area();
    let plan = layout::plan((screen.width, screen.height), area, app.look.gaps);

    let tiles = app.wall();
    strip::render(frame, plan.strip, app, &tiles, now);
    for (which, rect) in &plan.panels {
        match which {
            Panel::Inbox => inbox::render(frame, *rect, app, now),
            Panel::Wall => wall::render(frame, *rect, app, &tiles, plan.wall),
            Panel::Changes => changes::render(frame, *rect, app),
            Panel::Peek => peek::render(frame, *rect, app, now),
            other => extra::render(frame, *rect, app, *other, now),
        }
    }
}

/// `950`, `12.4k`, `3.1M`.
pub(crate) fn compact_count(v: f64) -> String {
    match v {
        v if v < 1_000.0 => format!("{v:.0}"),
        v if v < 1_000_000.0 => format!("{:.1}k", v / 1e3),
        v => format!("{:.1}M", v / 1e6),
    }
}

/// A rounded panel: the title inset at the left of the top border, meta at
/// the right.
pub(crate) fn panel<'a>(
    t: &Theme,
    icon: &str,
    title: &str,
    meta: Line<'a>,
    border: Color,
) -> Block<'a> {
    let name = Line::from(vec![
        Span::styled(format!(" {icon} "), Style::new().fg(t.label)),
        Span::styled(format!("{title} "), Style::new().fg(t.title)),
    ]);
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border))
        .style(Style::new().bg(t.panel_bg))
        .title(name)
        .title(meta.right_aligned())
}

/// The mark that stands for a session: its name's first letter.
pub(crate) fn sigil(name: &str) -> char {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map_or('·', |c| c.to_ascii_uppercase())
}

/// `45s`, `12m`, `3h05`: ages in the narrow columns.
pub(crate) fn age(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h{:02}", s / 3600, s % 3600 / 60),
        _ => format!("{}d", s / 86_400),
    }
}

/// The glyph for `icon` in the current set.
pub(crate) fn glyph(app: &App, icon: Icon) -> &'static str {
    app.look.icons.get(icon)
}
