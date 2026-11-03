//! Where Focus puts its panels at each window size. Pure functions of the
//! window and the body rectangle, like Mission control's, so every density
//! is testable without drawing.

use ratatui::layout::{Constraint, Layout, Rect};

use crate::ui::mission::layout::{Density, density};

/// Smallest window Focus lays out in.
pub const MIN_SIZE: (u16, u16) = (60, 14);
/// From this body width the stream, the answer and the reviewer sit side by
/// side; below it the answer goes under the stream.
pub const SIDE_BY_SIDE: u16 = 100;

const HEADER_H: u16 = 2;
const SAT_H: u16 = 7;
const LANE_H: u16 = 4;
/// Rows the stream and answer need before the strips above give way.
const MAIN_MIN: u16 = 6;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub density: Density,
    pub header: Rect,
    pub sat: Rect,
    pub lane: Rect,
    pub timeline: Rect,
    pub stream: Rect,
    pub answer: Rect,
    pub review: Option<Rect>,
    /// The todo list and the agents, from the full density up.
    pub plan: Option<Rect>,
    pub agents: Option<Rect>,
}

fn timeline_h(d: Density) -> u16 {
    match d {
        Density::Compact => 4,
        Density::Standard => 7,
        Density::Full => 9,
        Density::Ultrawide => 11,
    }
}

/// Lays out `body` for a window of `screen` columns and rows. The review
/// column is only given room when `review` says a review is used.
pub fn plan(screen: (u16, u16), body: Rect, gap: u16, review: bool) -> Plan {
    let d = density(screen.0, screen.1);
    // Strips give way from the bottom up when the window is short.
    let mut heights = [HEADER_H, SAT_H, LANE_H, timeline_h(d)];
    let mut room = body.height.saturating_sub(MAIN_MIN);
    for h in &mut heights {
        *h = (*h).min(room);
        room -= *h;
    }
    let [header, sat, lane, timeline, main] = Layout::vertical([
        Constraint::Length(heights[0]),
        Constraint::Length(heights[1]),
        Constraint::Length(heights[2]),
        Constraint::Length(heights[3]),
        Constraint::Fill(1),
    ])
    .areas(body);

    let mut out = Plan {
        density: d,
        header,
        sat,
        lane,
        timeline,
        stream: main,
        answer: Rect::ZERO,
        review: None,
        plan: None,
        agents: None,
    };
    if main.width < SIDE_BY_SIDE {
        let [stream, answer] = Layout::vertical([Constraint::Percentage(60), Constraint::Fill(1)])
            .spacing(gap)
            .areas(main);
        out.stream = stream;
        out.answer = answer;
        return out;
    }
    let (answer_w, side_w) = match d {
        Density::Compact => (36, 30),
        Density::Standard => (52, 42),
        Density::Full => (70, 50),
        Density::Ultrawide => (84, 64),
    };
    let side = review || d >= Density::Full;
    let cols = if side {
        vec![
            Constraint::Fill(1),
            Constraint::Length(answer_w),
            Constraint::Length(side_w),
        ]
    } else {
        vec![
            Constraint::Fill(1),
            Constraint::Length(answer_w + side_w / 2),
        ]
    };
    let areas = Layout::horizontal(cols).spacing(gap).split(main);
    out.stream = areas[0];
    out.answer = areas[1];
    if side {
        let column = areas[2];
        if d >= Density::Full {
            let plan_h = 12.min(column.height / 3).max(3);
            let agents_h = 8.min(column.height / 4).max(3);
            let rows = if review {
                vec![
                    Constraint::Fill(1),
                    Constraint::Length(plan_h),
                    Constraint::Length(agents_h),
                ]
            } else {
                vec![
                    Constraint::Length(0),
                    Constraint::Fill(1),
                    Constraint::Length(agents_h),
                ]
            };
            let parts = Layout::vertical(rows).spacing(gap).split(column);
            out.review = review.then_some(parts[0]);
            out.plan = Some(parts[1]);
            out.agents = Some(parts[2]);
        } else {
            out.review = Some(column);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(w: u16, h: u16) -> Rect {
        Rect::new(0, 1, w, h.saturating_sub(3))
    }

    fn inside(inner: Rect, outer: Rect) -> bool {
        inner.is_empty() || outer.contains((inner.x, inner.y).into())
    }

    #[test]
    fn densities_follow_mission_control() {
        assert_eq!(
            plan((120, 36), area(120, 36), 1, false).density,
            Density::Compact
        );
        assert_eq!(
            plan((160, 48), area(160, 48), 1, false).density,
            Density::Standard
        );
        assert_eq!(
            plan((240, 64), area(240, 64), 1, false).density,
            Density::Full
        );
        assert_eq!(
            plan((320, 90), area(320, 90), 1, false).density,
            Density::Ultrawide
        );
    }

    #[test]
    fn the_reviewer_column_exists_only_when_a_review_is_used() {
        for size in [(120, 36), (160, 48), (240, 64)] {
            let without = plan(size, area(size.0, size.1), 1, false);
            let with = plan(size, area(size.0, size.1), 1, true);
            assert!(without.review.is_none(), "{size:?}");
            assert!(with.review.is_some(), "{size:?}");
            assert!(with.stream.width < without.stream.width + without.answer.width);
        }
    }

    #[test]
    fn plan_and_agents_appear_from_the_full_density() {
        let std = plan((160, 48), area(160, 48), 1, true);
        assert!(std.plan.is_none() && std.agents.is_none());
        let full = plan((240, 64), area(240, 64), 1, false);
        assert!(full.plan.is_some() && full.agents.is_some());
    }

    #[test]
    fn narrow_windows_stack_the_answer_under_the_stream() {
        let p = plan((90, 30), area(90, 30), 1, true);
        assert!(p.review.is_none());
        assert!(p.answer.y >= p.stream.bottom());
        assert_eq!(p.stream.width, p.answer.width);
    }

    #[test]
    fn nothing_leaves_the_body_at_any_size() {
        for w in 0..=330u16 {
            for h in (0..=90u16).step_by(3) {
                for review in [false, true] {
                    let body = area(w, h);
                    let p = plan((w, h), body, 1, review);
                    for r in [p.header, p.sat, p.lane, p.timeline, p.stream, p.answer]
                        .into_iter()
                        .chain(p.review)
                        .chain(p.plan)
                        .chain(p.agents)
                    {
                        assert!(
                            r.right() <= body.right() && r.bottom() <= body.bottom(),
                            "{w}x{h} {r:?}"
                        );
                        let _ = inside(r, body);
                    }
                }
            }
        }
    }
}
