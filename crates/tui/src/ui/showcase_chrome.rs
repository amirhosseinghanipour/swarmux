//! What the showcase screens share: the header, the footer or the banner
//! that says why the screen stands still, and the dimming.

use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
};
use swarmux_domain::Priority;

use crate::app::App;
use crate::showcase::{Mode, Phase, Showcase};
use crate::theme::{Theme, mix};

/// How far a frozen screen is dimmed toward the background.
const FROZEN_DIM: f64 = 0.6;

/// What the chrome says about the screen.
pub struct Chrome {
    pub title: &'static str,
    pub other: (&'static str, char),
}

impl Chrome {
    pub fn new(app: &App, title: &'static str) -> Chrome {
        let other = match app.showcase.mode {
            Some(Mode::Spectrum) => ("constellation", 'z'),
            _ => ("spectrum", 'S'),
        };
        Chrome { title, other }
    }
}

/// The header row, the body and the footer row.
pub fn split(area: Rect) -> (Rect, Rect, Rect) {
    let [head, body, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    (head, body, foot)
}

pub fn header(frame: &mut Frame, area: Rect, app: &App, chrome: &Chrome) {
    let t = &app.look.theme;
    let show = &app.showcase;
    let running = show
        .agents
        .iter()
        .filter(|a| a.phase == Phase::Working)
        .count();
    let waiting = show
        .agents
        .iter()
        .filter(|a| a.phase == Phase::Waiting)
        .count();
    let mut right = format!("{} agents · {running} running", show.agents.len());
    if waiting > 0 {
        right.push_str(&format!(" · {waiting} waiting"));
    }
    if let Some(spend) = show.spend_today {
        right.push_str(&format!(" · ${spend:.2} today"));
    }
    let left = Line::from(vec![
        Span::styled(
            "SWARMUX",
            Style::new().fg(t.title).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  ·  {}", chrome.title), Style::new().fg(t.lead)),
    ]);
    frame.render_widget(left, area);
    frame.render_widget(
        Line::from(Span::styled(right, Style::new().fg(t.muted))).right_aligned(),
        area,
    );
}

pub fn footer(frame: &mut Frame, area: Rect, app: &App, chrome: &Chrome) {
    let t = &app.look.theme;
    if let Some(alert) = &app.showcase.alert {
        let bg = if alert.priority == Priority::P1 {
            t.error
        } else {
            t.warn
        };
        let text = format!(
            " {} ▸ {}  ·  held still · y/n answer a held call · 1 mission control ",
            alert.priority.as_str(),
            alert.text
        );
        let line = Line::from(Span::styled(
            text,
            Style::new()
                .fg(t.on_accent)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(line, area);
        return;
    }
    let keys = format!(
        "z/esc leave · {} {} · t theme · 1-6 screens · q quit",
        chrome.other.1, chrome.other.0
    );
    frame.render_widget(Line::from(Span::styled(keys, Style::new().fg(t.dim))), area);
    let tier = app.look.tier();
    if tier != crate::ui::motion::Motion::Full {
        let why = app
            .look
            .motion_reason()
            .map_or(String::new(), |r| format!(" ({})", r.as_str()));
        frame.render_widget(
            Line::from(Span::styled(
                format!("motion {}{why}", tier.as_str()),
                Style::new().fg(t.muted),
            ))
            .right_aligned(),
            area,
        );
    }
}

/// Dims `area` toward the background when the screens are held still. What
/// is drawn in the warning colour, the star that needs the user, stays lit.
pub fn dim_when_frozen(buf: &mut Buffer, area: Rect, t: &Theme, show: &Showcase) {
    if !show.frozen() {
        return;
    }
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y))
                && cell.fg != t.warn
            {
                cell.fg = mix(cell.fg, t.bg, FROZEN_DIM);
            }
        }
    }
}
