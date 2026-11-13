//! Work: decide and land. The ranked queue at the left, the selected
//! outcome's work product at the right, and the actions it allows.
//!
//! Drawn from the console snapshot the app builds when what it reads
//! changes; drawing does no I/O.

mod product;
mod queue;
mod right;
mod util;

#[cfg(test)]
mod tests;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::Line,
    widgets::Paragraph,
};

use crate::app::App;

/// The window below which Work shows only how to enlarge it.
const MIN_WIDTH: u16 = 80;
const MIN_HEIGHT: u16 = 16;

/// The queue's width for a body `width` cells wide: a third of it, within
/// what the longest rows of the queue need and what leaves the right pane
/// room.
fn queue_width(width: u16) -> u16 {
    (width / 3).clamp(40, 62)
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        too_small(frame, area, app);
        return;
    }
    let Some(snap) = app.console_current() else {
        frame.render_widget(
            Paragraph::new(Line::from("reading the queue…").style(Style::new().fg(t.muted))),
            area,
        );
        return;
    };
    render_with(frame, area, app, snap);
}

/// Draws `snap` as the queue and the outcome selected in it. The tests feed
/// it a snapshot of their own.
pub(crate) fn render_with(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    snap: &swarmux_application::console::ConsoleSnapshot,
) {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        too_small(frame, area, app);
        return;
    }
    // The approval, consent and typed-confirm overlays sit on the bottom
    // rows of the body: leave them room, so no pane is hidden under them.
    let reserved = if matches!(app.mode, crate::app::Mode::Confirm(_)) {
        crate::ui::confirm::HEIGHT.min(area.height)
    } else if app.engine.guard.jev().is_some_and(|j| j.wants_consent())
        || app.approval_target().is_some()
    {
        crate::ui::overlay_rows(app, area.height)
    } else {
        0
    };
    let area = Rect {
        height: area.height.saturating_sub(reserved),
        ..area
    };
    let [left, rule, right] = Layout::horizontal([
        Constraint::Length(queue_width(area.width)),
        Constraint::Length(3),
        Constraint::Fill(1),
    ])
    .areas(area);

    let current = app.work_screen.current(snap);
    queue::render(frame, left, app, snap, current.map(|o| o.id.as_str()));
    // A rule between the two, in the border colour.
    let bar = util::rule_column(app);
    let lines: Vec<Line> = (0..rule.height)
        .map(|_| Line::from(bar.clone()).centered())
        .collect();
    frame.render_widget(Paragraph::new(lines), rule);
    right::render(frame, right, app, snap, current);
}

/// Asks for a window the layout fits in.
fn too_small(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    frame.render_widget(
        Paragraph::new(vec![
            Line::from("make the window bigger").style(Style::new().fg(t.label)),
            Line::from("work needs 80×16").style(Style::new().fg(t.dim)),
        ])
        .centered(),
        area,
    );
}
