//! Where Mission control's panels go at each window size. Pure functions
//! of the window and the body rectangle, so every tier is testable without
//! drawing. Panels are listed by priority: as the room shrinks, the last
//! ones in a stack or row are the first to go.

use ratatui::layout::{Constraint, Layout, Rect};

/// How much of the screen is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Density {
    /// Up to about 120×36: limits strip, inbox, session list.
    Compact,
    /// About 160×48: adds the wall, the change log and the token flow.
    Standard,
    /// From 220×60: adds a second layer of real-data panels.
    Full,
    /// From 300×80: adds a focus peek of the selected agent.
    Ultrawide,
}

/// The tiers' lower bounds on the window (columns, rows).
pub const STANDARD_AT: (u16, u16) = (140, 40);
pub const FULL_AT: (u16, u16) = (220, 60);
pub const ULTRAWIDE_AT: (u16, u16) = (300, 80);

/// Width of the focus peek column.
const PEEK_WIDTH: u16 = 56;
/// Height of the lower band in the full tier, borders included.
const BAND_HEIGHT: u16 = 14;
/// Height of the change log row in the standard tier.
const FOOTER_HEIGHT: u16 = 8;

pub fn density(width: u16, height: u16) -> Density {
    let at = |(w, h): (u16, u16)| width >= w && height >= h;
    if at(ULTRAWIDE_AT) {
        Density::Ultrawide
    } else if at(FULL_AT) {
        Density::Full
    } else if at(STANDARD_AT) {
        Density::Standard
    } else {
        Density::Compact
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Panel {
    Inbox,
    Wall,
    Changes,
    /// Token flow: in, out and cache per minute.
    Flow,
    Burn,
    Mix,
    Agents,
    Tools,
    Files,
    Git,
    Ci,
    Mcp,
    System,
    Calendar,
    Peek,
}

/// How the wall draws sessions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WallMode {
    /// One row per session.
    List,
    /// Cards of `height` rows in stable slots; at most `cap` of them, and
    /// free slots drawn faintly when `slots` is set.
    Cards {
        height: u16,
        cap: usize,
        slots: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub density: Density,
    pub strip: Rect,
    pub panels: Vec<(Panel, Rect)>,
    pub wall: WallMode,
}

impl Plan {
    #[cfg(test)]
    pub fn area(&self, panel: Panel) -> Option<Rect> {
        self.panels
            .iter()
            .find(|(p, _)| *p == panel)
            .map(|(_, r)| *r)
    }
}

/// Lays out `body` for a window of `screen` columns and rows.
pub fn plan(screen: (u16, u16), body: Rect, gap: u16) -> Plan {
    let density = density(screen.0, screen.1);
    let mut panels = Vec::new();
    let mut main = body;
    if density == Density::Ultrawide && body.width > PEEK_WIDTH * 2 {
        let [left, peek] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(PEEK_WIDTH)])
                .spacing(gap)
                .areas(body);
        panels.push((Panel::Peek, peek));
        main = left;
    }
    let strip_h = (if main.width >= 100 { 3 } else { 4 }).min(main.height);
    let [strip, rest] = Layout::vertical([Constraint::Length(strip_h), Constraint::Fill(1)])
        .spacing(0)
        .areas(main);

    let wall = match density {
        Density::Compact => WallMode::List,
        Density::Standard => WallMode::Cards {
            height: 6,
            cap: 8,
            slots: false,
        },
        Density::Full | Density::Ultrawide => WallMode::Cards {
            height: 8,
            cap: 12,
            slots: true,
        },
    };

    match density {
        Density::Compact => {
            let (l, r) = if rest.width >= 120 {
                let [l, r] = Layout::horizontal([Constraint::Percentage(40), Constraint::Fill(1)])
                    .spacing(gap)
                    .areas(rest);
                (l, r)
            } else {
                let [l, r] = Layout::vertical([Constraint::Percentage(50), Constraint::Fill(1)])
                    .spacing(gap)
                    .areas(rest);
                (l, r)
            };
            panels.extend([(Panel::Inbox, l), (Panel::Wall, r)]);
        }
        Density::Standard => {
            let footer = FOOTER_HEIGHT.min(rest.height / 3);
            let [middle, bottom] =
                Layout::vertical([Constraint::Fill(1), Constraint::Length(footer)])
                    .spacing(gap)
                    .areas(rest);
            let [l, r] = Layout::horizontal([Constraint::Percentage(40), Constraint::Fill(1)])
                .spacing(gap)
                .areas(middle);
            panels.extend([(Panel::Inbox, l), (Panel::Wall, r)]);
            if footer >= 4 {
                let [c, f] = Layout::horizontal([Constraint::Percentage(60), Constraint::Fill(1)])
                    .spacing(gap)
                    .areas(bottom);
                panels.extend([(Panel::Changes, c), (Panel::Flow, f)]);
            }
        }
        Density::Full | Density::Ultrawide => {
            let band = if rest.height >= BAND_HEIGHT + 20 {
                BAND_HEIGHT
            } else {
                0
            };
            let [top, bottom] = Layout::vertical([Constraint::Fill(1), Constraint::Length(band)])
                .spacing(if band > 0 { gap } else { 0 })
                .areas(rest);
            let [inbox, wall_a, col_a, col_b] = Layout::horizontal([
                Constraint::Percentage(24),
                Constraint::Percentage(38),
                Constraint::Percentage(19),
                Constraint::Fill(1),
            ])
            .spacing(gap)
            .areas(top);
            panels.extend([(Panel::Inbox, inbox), (Panel::Wall, wall_a)]);
            panels.extend(stack(
                col_a,
                &[
                    (Panel::Burn, 5, 9),
                    (Panel::Mix, 6, 9),
                    (Panel::System, 5, 0),
                ],
                gap,
            ));
            panels.extend(stack(
                col_b,
                &[
                    (Panel::Agents, 5, 10),
                    (Panel::Flow, 5, 8),
                    (Panel::Mcp, 4, 6),
                    (Panel::Calendar, 9, 0),
                ],
                gap,
            ));
            if band > 0 {
                panels.extend(row(
                    bottom,
                    &[
                        (Panel::Changes, 30, 3),
                        (Panel::Tools, 24, 2),
                        (Panel::Files, 22, 2),
                        (Panel::Git, 24, 2),
                        (Panel::Ci, 22, 2),
                    ],
                    gap,
                ));
            }
        }
    }
    Plan {
        density,
        strip,
        panels,
        wall,
    }
}

/// Panels stacked top to bottom as `(panel, min height, wanted height)`;
/// a wanted height of 0 takes what is left. Panels that don't fit at their
/// minimum are left out, last first.
fn stack(area: Rect, items: &[(Panel, u16, u16)], gap: u16) -> Vec<(Panel, Rect)> {
    let mut used = 0u16;
    let mut kept = Vec::new();
    for item in items {
        let need = item.1 + if kept.is_empty() { 0 } else { gap };
        if used + need > area.height {
            break;
        }
        used += need;
        kept.push(*item);
    }
    let mut spare = area.height - used;
    let mut heights: Vec<u16> = kept.iter().map(|i| i.1).collect();
    for (h, item) in heights.iter_mut().zip(&kept) {
        let want = if item.2 == 0 { u16::MAX } else { item.2 };
        let add = want.saturating_sub(item.1).min(spare);
        *h += add;
        spare -= add;
    }
    if let Some(last) = heights.last_mut() {
        *last += spare;
    }
    let mut y = area.y;
    kept.iter()
        .zip(heights)
        .map(|(item, h)| {
            let r = Rect::new(area.x, y, area.width, h);
            y += h + gap;
            (item.0, r)
        })
        .collect()
}

/// Panels side by side as `(panel, min width, weight)`; the ones that don't
/// fit at their minimum are left out, last first.
fn row(area: Rect, items: &[(Panel, u16, u16)], gap: u16) -> Vec<(Panel, Rect)> {
    let mut used = 0u16;
    let mut kept = Vec::new();
    for item in items {
        let need = item.1 + if kept.is_empty() { 0 } else { gap };
        if used + need > area.width {
            break;
        }
        used += need;
        kept.push(*item);
    }
    if kept.is_empty() {
        return Vec::new();
    }
    let rects = Layout::horizontal(kept.iter().map(|i| Constraint::Fill(i.2)))
        .spacing(gap)
        .split(area);
    kept.iter()
        .zip(rects.iter())
        .map(|(i, r)| (i.0, *r))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_of(w: u16, h: u16) -> Rect {
        Rect::new(0, 2, w, h.saturating_sub(3))
    }

    fn panels(w: u16, h: u16) -> Plan {
        plan((w, h), body_of(w, h), 1)
    }

    fn has(p: &Plan, panel: Panel) -> bool {
        p.area(panel).is_some()
    }

    #[test]
    fn tiers_start_at_their_bounds() {
        assert_eq!(density(120, 36), Density::Compact);
        assert_eq!(density(139, 60), Density::Compact);
        assert_eq!(density(160, 48), Density::Standard);
        assert_eq!(density(240, 59), Density::Standard);
        assert_eq!(density(240, 64), Density::Full);
        assert_eq!(density(299, 80), Density::Full);
        assert_eq!(density(300, 80), Density::Ultrawide);
        assert_eq!(density(300, 79), Density::Full);
    }

    #[test]
    fn each_tier_adds_panels_to_the_one_below() {
        let c = panels(120, 36);
        assert_eq!(c.density, Density::Compact);
        assert!(has(&c, Panel::Inbox) && has(&c, Panel::Wall));
        assert!(!has(&c, Panel::Changes) && !has(&c, Panel::Flow));
        assert_eq!(c.wall, WallMode::List);

        let s = panels(160, 48);
        for p in [Panel::Inbox, Panel::Wall, Panel::Changes, Panel::Flow] {
            assert!(has(&s, p), "{p:?}");
        }
        assert!(!has(&s, Panel::Burn) && !has(&s, Panel::Peek));

        let f = panels(240, 64);
        for p in [
            Panel::Inbox,
            Panel::Wall,
            Panel::Changes,
            Panel::Flow,
            Panel::Burn,
            Panel::Mix,
            Panel::Agents,
            Panel::Tools,
            Panel::Files,
            Panel::Git,
            Panel::Ci,
            Panel::Mcp,
            Panel::System,
            Panel::Calendar,
        ] {
            assert!(has(&f, p), "{p:?}");
        }
        assert!(!has(&f, Panel::Peek));
        assert!(matches!(
            f.wall,
            WallMode::Cards {
                cap: 12,
                slots: true,
                ..
            }
        ));

        let u = panels(300, 80);
        assert_eq!(u.density, Density::Ultrawide);
        assert!(has(&u, Panel::Peek) && has(&u, Panel::Calendar));
        let peek = u.area(Panel::Peek).unwrap();
        assert_eq!(peek.right(), 300, "the peek hugs the right edge");
    }

    #[test]
    fn panels_stay_inside_the_body_and_never_overlap() {
        for w in (0..=330).step_by(3) {
            for h in (0..=90).step_by(2) {
                let body = body_of(w, h);
                let p = plan((w, h), body, 1);
                let all: Vec<Rect> = std::iter::once(p.strip)
                    .chain(p.panels.iter().map(|(_, r)| *r))
                    .collect();
                for r in &all {
                    assert!(
                        r.x >= body.x
                            && r.y >= body.y
                            && r.right() <= body.right()
                            && r.bottom() <= body.bottom(),
                        "{w}x{h}: {r:?} outside {body:?}"
                    );
                }
                for (i, a) in all.iter().enumerate() {
                    for b in &all[i + 1..] {
                        let overlap = a.width > 0
                            && a.height > 0
                            && b.width > 0
                            && b.height > 0
                            && a.intersects(*b);
                        assert!(!overlap, "{w}x{h}: {a:?} overlaps {b:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn second_layer_collapses_by_priority() {
        // A full-width but short window is only standard: no second layer.
        let p = panels(260, 50);
        assert_eq!(p.density, Density::Standard);
        assert!(!has(&p, Panel::Burn));
        // At the edge of full the lowest priorities are the first to go.
        let tight = plan((220, 60), Rect::new(0, 2, 220, 40), 1);
        assert!(has(&tight, Panel::Burn));
        let col = |p: &Plan, panel| p.area(panel).map(|r| r.height);
        assert!(col(&tight, Panel::Calendar).is_none_or(|h| h >= 9));
    }

    #[test]
    fn stacks_give_leftovers_to_the_last_panel() {
        let out = stack(
            Rect::new(0, 0, 10, 30),
            &[
                (Panel::Burn, 5, 9),
                (Panel::Mix, 6, 9),
                (Panel::System, 5, 0),
            ],
            1,
        );
        let heights: Vec<u16> = out.iter().map(|(_, r)| r.height).collect();
        assert_eq!(heights, [9, 9, 10]);
        let small = stack(
            Rect::new(0, 0, 10, 12),
            &[(Panel::Burn, 5, 9), (Panel::Mix, 6, 9)],
            1,
        );
        assert_eq!(small.len(), 2);
        assert_eq!(small[1].1.bottom(), 12);
        let one = stack(
            Rect::new(0, 0, 10, 8),
            &[(Panel::Burn, 5, 9), (Panel::Mix, 6, 9)],
            1,
        );
        assert_eq!(one.len(), 1);
    }
}
