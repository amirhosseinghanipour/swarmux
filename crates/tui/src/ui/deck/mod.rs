//! The Deck: the task graph in the middle, and around it what the work
//! needs from the user, how verification is going, what the limits allow,
//! what the Guard held, and what happened lately.
//!
//! Drawn from the console snapshot the app builds when what it reads
//! changes; drawing does no I/O. Which panels show and in which order is
//! the user's `[deck]`; at a small size some fold into the graph's place
//! (see [`arrange`]).

mod graph;
mod panels;
mod sessions;
#[cfg(test)]
mod tests;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph},
};
use swarmux_application::console::ConsoleSnapshot;
use swarmux_domain::console::{DeckLayout, Panel, Zone};

use crate::app::App;
use crate::ui::work::util::fit;

/// The window below which the Deck shows only how to enlarge it.
const MIN_WIDTH: u16 = 80;
const MIN_HEIGHT: u16 = 16;
/// Below either of these the Deck folds panels: the graph stays, and some
/// panels take turns in its place.
pub const WIDE_WIDTH: u16 = 140;
pub const WIDE_HEIGHT: u16 = 40;

/// The panels that stay on screen when the Deck folds.
const KEPT_WHEN_FOLDED: [Panel; 3] = [Panel::Needs, Panel::Limits, Panel::Guard];
/// The panels that fold, in the order `Tab` brings them.
pub const FOLDABLE: [Panel; 3] = [Panel::Sessions, Panel::Verification, Panel::Timeline];

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let Some(snap) = app.console_current() else {
        frame.render_widget(
            Paragraph::new(Line::from("reading the work…").style(Style::new().fg(t.muted))),
            area,
        );
        return;
    };
    render_with(frame, area, app, snap);
}

/// Draws `snap` as the Deck. The tests feed it a snapshot of their own.
pub(crate) fn render_with(frame: &mut Frame, area: Rect, app: &App, snap: &ConsoleSnapshot) {
    let t = &app.look.theme;
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from("make the window bigger").style(Style::new().fg(t.label)),
                Line::from(format!("the deck needs {MIN_WIDTH}×{MIN_HEIGHT}"))
                    .style(Style::new().fg(t.dim)),
            ])
            .centered(),
            area,
        );
        return;
    }
    // The approval, consent and typed-confirm overlays sit on the bottom
    // rows of the body: leave them room, so no panel is hidden under them.
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
    let layout = app.deck_layout();
    // What could not be used in `[deck]` is said, for as long as it is
    // wrong: one line above the panels.
    let body = if layout.problems.is_empty() {
        area
    } else {
        let [warn, rest] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        let first = &layout.problems[0];
        let more = layout.problems.len() - 1;
        let text = format!(
            " ! {first}{} · run `swarmux deck lint`",
            if more > 0 {
                format!(" (+{more} more)")
            } else {
                String::new()
            }
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                fit(&text, usize::from(warn.width)),
                Style::new().fg(t.warn),
            ))),
            warn,
        );
        rest
    };
    for (panel, rect) in arrange(
        body,
        layout,
        app.deck_screen.folded,
        folds(body.width, body.height),
    ) {
        match panel {
            Panel::Graph => graph::render(frame, rect, app, snap),
            Panel::Sessions => sessions::render(frame, rect, app, snap),
            other => panels::render(frame, rect, app, snap, other),
        }
    }
}

/// Whether panels fold at this size.
pub fn folds(width: u16, height: u16) -> bool {
    width < WIDE_WIDTH || height < WIDE_HEIGHT
}

/// The panels of the Deck in the order `Tab` brings the folded ones, for
/// the screen to say which it is on.
pub fn foldable(layout: &DeckLayout) -> Vec<Panel> {
    FOLDABLE.into_iter().filter(|p| layout.shows(*p)).collect()
}

/// The rows each of `wants` gets out of `height`: each its minimum, then
/// each up to its ideal in order, then what is left to the ones that grow,
/// by weight. When even the minimums do not fit, the tallest give up rows.
pub fn heights(height: u16, wants: &[Want]) -> Vec<u16> {
    let mut got: Vec<u16> = wants.iter().map(|w| w.min).collect();
    let mut spare = i32::from(height) - i32::from(got.iter().sum::<u16>());
    if spare < 0 {
        while spare < 0 && got.iter().any(|g| *g > 3) {
            // The tallest first; the earlier one on a tie.
            let (i, _) = got
                .iter()
                .enumerate()
                .max_by_key(|(i, g)| (**g, std::cmp::Reverse(*i)))
                .unwrap_or((0, &0));
            got[i] -= 1;
            spare += 1;
        }
        return got;
    }
    for (g, w) in got.iter_mut().zip(wants) {
        let add = i32::from(w.ideal.saturating_sub(w.min)).min(spare);
        *g += u16::try_from(add).unwrap_or(0);
        spare -= add;
    }
    let weight: u32 = wants.iter().map(|w| u32::from(w.grow)).sum();
    if spare > 0 && weight > 0 {
        let spare_u = u32::try_from(spare).unwrap_or(0);
        let mut given = 0u32;
        let mut last = None;
        for (g, w) in got.iter_mut().zip(wants) {
            if w.grow == 0 {
                continue;
            }
            let share = spare_u * u32::from(w.grow) / weight;
            *g += u16::try_from(share).unwrap_or(0);
            given += share;
            last = Some(g);
        }
        // What integer division left goes to the last one that grows.
        if let Some(g) = last {
            *g += u16::try_from(spare_u - given).unwrap_or(0);
        }
    }
    got
}

/// What a panel asks of a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Want {
    pub min: u16,
    pub ideal: u16,
    /// How much of the rows left over it takes, relative to the others.
    pub grow: u16,
}

fn want(panel: Panel, layout: &DeckLayout) -> Want {
    match panel {
        Panel::Graph => Want {
            min: 12,
            ideal: 12,
            grow: 4,
        },
        // Border, header and the rows the user asked for.
        Panel::Sessions => {
            let rows = u16::try_from(layout.session_rows).unwrap_or(7);
            Want {
                min: 5,
                ideal: rows + 3,
                grow: 0,
            }
        }
        Panel::Needs => Want {
            min: 6,
            ideal: 9,
            grow: 1,
        },
        Panel::Verification => Want {
            min: 8,
            ideal: 9,
            grow: 0,
        },
        Panel::Limits => Want {
            min: 9,
            ideal: 15,
            grow: 2,
        },
        Panel::Guard | Panel::Timeline => Want {
            min: 6,
            ideal: 9,
            grow: 1,
        },
    }
}

fn stack(area: Rect, panels: &[Panel], layout: &DeckLayout) -> Vec<(Panel, Rect)> {
    let wants: Vec<Want> = panels.iter().map(|p| want(*p, layout)).collect();
    let mut y = area.y;
    panels
        .iter()
        .zip(heights(area.height, &wants))
        .map(|(p, h)| {
            let r = Rect {
                y,
                height: h,
                ..area
            };
            y += h;
            (*p, r)
        })
        .collect()
}

/// Where each panel goes in `area`.
///
/// The panels of the config fall in three zones ([`Panel::zone`]): the
/// main column at the left (graph, sessions), the side column at the right
/// (needs, verification, limits) and the foot along the bottom (guard,
/// timeline). The order of `panels` is the order within a zone, top to
/// bottom or left to right. A zone with no panel gives its room to the
/// others.
///
/// When `fold` (the window is under 140×40) only the graph, the user's
/// needs, the limits and the Guard stay; the sessions, verification and the
/// timeline take turns in the graph's place (`folded` says which; none is
/// the graph itself). The graph always stays reachable.
pub fn arrange(
    area: Rect,
    layout: &DeckLayout,
    folded: Option<Panel>,
    fold: bool,
) -> Vec<(Panel, Rect)> {
    let zone = |z: Zone| -> Vec<Panel> {
        layout
            .zone(z)
            .into_iter()
            .filter(|p| !fold || KEPT_WHEN_FOLDED.contains(p) || *p == Panel::Graph)
            .collect()
    };
    let mut main = zone(Zone::Main);
    let side = zone(Zone::Side);
    let foot = zone(Zone::Foot);
    if fold {
        // One panel takes the place of the graph: a folded one when the
        // user brought it, else the graph. With no graph in the config the
        // first folded panel stands there.
        let wanted = folded
            .filter(|p| layout.shows(*p) && FOLDABLE.contains(p))
            .or_else(|| {
                if main.contains(&Panel::Graph) {
                    None
                } else {
                    foldable(layout).first().copied()
                }
            });
        main = match wanted {
            Some(p) => vec![p],
            None => main,
        };
    }

    let mut out = Vec::new();
    let foot_h = if foot.is_empty() {
        0
    } else if fold {
        (area.height / 4).clamp(6, 9)
    } else {
        (area.height / 5).clamp(8, 11)
    };
    let [top, bottom] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(foot_h)]).areas(area);
    let (main_rect, side_rect) = match (main.is_empty(), side.is_empty()) {
        (true, true) => (None, None),
        (false, true) => (Some(top), None),
        (true, false) => (None, Some(top)),
        (false, false) => {
            let side_w = if fold {
                (top.width * 36 / 100).clamp(36, 52)
            } else {
                (top.width * 28 / 100).clamp(44, 60)
            };
            let [l, r] =
                Layout::horizontal([Constraint::Fill(1), Constraint::Length(side_w)]).areas(top);
            (Some(l), Some(r))
        }
    };
    if let Some(r) = main_rect {
        out.extend(stack(r, &main, layout));
    }
    if let Some(r) = side_rect {
        out.extend(stack(r, &side, layout));
    }
    if foot_h > 0 {
        let n = foot.len();
        let rects = if n == 1 {
            vec![bottom]
        } else {
            let left = bottom.width * 55 / 100;
            let [a, b] =
                Layout::horizontal([Constraint::Length(left), Constraint::Fill(1)]).areas(bottom);
            vec![a, b]
        };
        out.extend(foot.into_iter().zip(rects));
    }
    out
}

/// The frame of a panel: its title at the left in bold, a note at the
/// right, and a border of `color`.
pub(super) fn frame(
    app: &App,
    title: &str,
    note: &str,
    color: ratatui::style::Color,
) -> Block<'static> {
    let t = &app.look.theme;
    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(color))
        .style(Style::new().bg(t.panel_paint()))
        .title(Line::from(Span::styled(
            format!(" {title} "),
            Style::new().fg(t.title).add_modifier(Modifier::BOLD),
        )));
    if !note.is_empty() {
        block = block.title(
            Line::from(Span::styled(format!(" {note} "), Style::new().fg(t.muted))).right_aligned(),
        );
    }
    block
}
