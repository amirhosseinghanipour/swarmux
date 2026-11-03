//! Layouts as data: a tree of rows and columns of panels, with variants
//! for smaller windows. `command` is the reference dashboard, `btop` a
//! dense grid of small panels, `minimal` the lead and the log. Users add
//! their own with `[[layouts]]`.

use std::sync::OnceLock;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
};
use swarmux_application::LayoutSettings;

use super::{PanelId, ViewModel, render_panel};
use crate::theme::Theme;

/// How much of its parent a child takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    /// Exactly this many cells.
    Len(u16),
    /// A share of what is left.
    Fill(u16),
    /// A percentage of the parent.
    Pct(u16),
    /// At least this many cells, growing into what is left.
    Min(u16),
    /// Up to this many cells.
    Max(u16),
    /// What its panels prefer, kept between `min` and `max`.
    Auto(u16, u16),
}

/// A panel, or children side by side (`Row`) or stacked (`Col`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Panel(PanelId),
    Row(Vec<(Size, Node)>),
    Col(Vec<(Size, Node)>),
}

/// One arrangement, used when the window is at least `min` (columns, rows).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variant {
    pub min: (u16, u16),
    pub root: Node,
}

/// A named layout: the first variant the window fits wins (the last one
/// is used for anything smaller).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutSpec {
    pub name: String,
    pub variants: Vec<Variant>,
}

impl LayoutSpec {
    /// Every panel in the largest variant, top-left first.
    pub fn panel_ids(&self) -> Vec<PanelId> {
        fn walk(node: &Node, out: &mut Vec<PanelId>) {
            match node {
                Node::Panel(id) => {
                    if !out.contains(id) {
                        out.push(*id);
                    }
                }
                Row(children) | Col(children) => {
                    for (_, c) in children {
                        walk(c, out);
                    }
                }
            }
        }
        let mut out = Vec::new();
        if let Some(v) = self.variants.first() {
            walk(&v.root, &mut out);
        }
        out
    }

    /// The variant for a `w`×`h` window.
    pub fn variant(&self, w: u16, h: u16) -> Option<&Variant> {
        self.variants
            .iter()
            .find(|v| w >= v.min.0 && h >= v.min.1)
            .or_else(|| self.variants.last())
    }
}

use Node::{Col, Row};
use PanelId as P;
use Size::{Auto, Fill, Len, Max, Min, Pct};

fn p(id: PanelId) -> Node {
    Node::Panel(id)
}

/// The four header strips of the reference dashboard.
fn heading(legend: bool) -> Vec<(Size, Node)> {
    let mut rows = Vec::new();
    if legend {
        rows.push((Len(1), p(P::Legend)));
    }
    rows.extend([
        (Len(1), p(P::Command)),
        (Len(1), p(P::Steps)),
        (Len(1), p(P::Target)),
    ]);
    rows
}

/// The reference dashboard, collapsing as the window shrinks.
fn command() -> LayoutSpec {
    let right = |guard: u16| {
        Col(vec![
            (Len(4), p(P::Lead)),
            (Len(guard), p(P::Guard)),
            (Len(2), p(P::Delegate)),
            (Fill(1), p(P::Subagents)),
            (Len(4), p(P::Back)),
        ])
    };
    let main = Row(vec![(Pct(28), p(P::Reviewer)), (Fill(1), right(8))]);
    let full = {
        let mut rows = heading(true);
        rows.push((Max(27), main));
        rows.push((
            Fill(1),
            Col(vec![
                (
                    Auto(7, 11),
                    Row(vec![(Pct(60), p(P::Timeline)), (Fill(1), p(P::Who))]),
                ),
                (Min(3), p(P::Log)),
            ]),
        ));
        Col(rows)
    };
    let no_who = {
        let mut rows = heading(true);
        rows.push((
            Max(27),
            Row(vec![(Pct(28), p(P::Reviewer)), (Fill(1), right(6))]),
        ));
        rows.push((
            Fill(1),
            Row(vec![(Pct(55), p(P::Timeline)), (Fill(1), p(P::Log))]),
        ));
        Col(rows)
    };
    let compact = {
        let mut rows = heading(true);
        rows.extend([
            (Len(1), p(P::Reviewer)),
            (Len(4), p(P::Lead)),
            (Len(5), p(P::Guard)),
            (Auto(3, 6), p(P::Subagents)),
            (Len(1), p(P::Back)),
            (
                Fill(1),
                Row(vec![(Pct(45), p(P::Timeline)), (Fill(1), p(P::Log))]),
            ),
        ]);
        Col(rows)
    };
    let minimal = {
        let mut rows = heading(false);
        rows.extend([
            (Len(1), p(P::Reviewer)),
            (Len(1), p(P::Lead)),
            (Len(1), p(P::Guard)),
            (Auto(3, 5), p(P::Subagents)),
            (Len(1), p(P::Back)),
            (Fill(1), p(P::Log)),
        ]);
        Col(rows)
    };
    let tiny = Col(vec![
        (Len(1), p(P::Steps)),
        (Len(1), p(P::Lead)),
        (Len(1), p(P::Guard)),
        (Len(1), p(P::Reviewer)),
        (Fill(1), p(P::Log)),
        (Len(2), p(P::SizeHint)),
    ]);
    LayoutSpec {
        name: "command".into(),
        variants: vec![
            Variant {
                min: (150, 45),
                root: full,
            },
            Variant {
                min: (120, 38),
                root: no_who,
            },
            Variant {
                min: (100, 30),
                root: compact,
            },
            Variant {
                min: (80, 24),
                root: minimal,
            },
            Variant {
                min: (0, 0),
                root: tiny,
            },
        ],
    }
}

/// A dense grid of small panels.
fn btop() -> LayoutSpec {
    let wide = Col(vec![
        (Len(1), p(P::Steps)),
        (
            Fill(3),
            Row(vec![
                (Fill(3), p(P::Lead)),
                (Fill(3), p(P::Usage)),
                (Fill(4), p(P::Burn)),
                (Fill(3), p(P::Context)),
            ]),
        ),
        (
            Fill(5),
            Row(vec![
                (
                    Fill(3),
                    Col(vec![(Fill(1), p(P::Guard)), (Fill(1), p(P::Reviewer))]),
                ),
                (Fill(4), p(P::Subagents)),
                (
                    Fill(3),
                    Col(vec![(Fill(3), p(P::Tokens)), (Fill(2), p(P::Mix))]),
                ),
            ]),
        ),
        (
            Fill(3),
            Row(vec![
                (Fill(4), p(P::Timeline)),
                (Fill(3), p(P::Who)),
                (Fill(2), p(P::Tools)),
                (Fill(2), p(P::Files)),
            ]),
        ),
        (
            Fill(3),
            Row(vec![
                (Fill(2), p(P::System)),
                (Fill(2), p(P::Git)),
                (Fill(2), p(P::Ci)),
                (Fill(2), p(P::Todos)),
                (Fill(3), p(P::Calendar)),
            ]),
        ),
        (
            Fill(4),
            Row(vec![
                (Fill(5), p(P::Log)),
                (Fill(2), p(P::Agents)),
                (Fill(2), p(P::Wall)),
                (Fill(2), p(P::Env)),
            ]),
        ),
    ]);
    let narrow = Col(vec![
        (Len(1), p(P::Steps)),
        (
            Fill(2),
            Row(vec![(Fill(1), p(P::Lead)), (Fill(1), p(P::Usage))]),
        ),
        (
            Fill(3),
            Row(vec![(Fill(1), p(P::Guard)), (Fill(1), p(P::Subagents))]),
        ),
        (
            Fill(2),
            Row(vec![(Fill(1), p(P::Burn)), (Fill(1), p(P::Tokens))]),
        ),
        (
            Fill(2),
            Row(vec![(Fill(1), p(P::Timeline)), (Fill(1), p(P::System))]),
        ),
        (Fill(3), p(P::Log)),
    ]);
    LayoutSpec {
        name: "btop".into(),
        variants: vec![
            Variant {
                min: (140, 40),
                root: wide,
            },
            Variant {
                min: (0, 0),
                root: narrow,
            },
        ],
    }
}

/// The lead and the log.
fn minimal() -> LayoutSpec {
    LayoutSpec {
        name: "minimal".into(),
        variants: vec![Variant {
            min: (0, 0),
            root: Col(vec![
                (Len(1), p(P::Steps)),
                (Len(4), p(P::Lead)),
                (Fill(1), p(P::Log)),
            ]),
        }],
    }
}

/// The built-in layouts, in the order `L` cycles them.
pub fn presets() -> Vec<LayoutSpec> {
    vec![command(), btop(), minimal()]
}

/// The layout to use when none is configured.
pub fn fallback() -> &'static LayoutSpec {
    static FALLBACK: OnceLock<LayoutSpec> = OnceLock::new();
    FALLBACK.get_or_init(command)
}

/// A `[[layouts]]` entry as a layout.
pub fn from_settings(user: &LayoutSettings) -> Result<LayoutSpec, String> {
    let mut rows = Vec::new();
    for row in &user.rows {
        let mut cells = Vec::new();
        for cell in &row.panels {
            let (id, weight) = match cell.split_once(':') {
                Some((id, w)) => (
                    id,
                    w.trim()
                        .parse::<u16>()
                        .ok()
                        .filter(|w| (1..=100).contains(w))
                        .ok_or_else(|| format!("bad weight in \"{cell}\""))?,
                ),
                None => (cell.as_str(), 1),
            };
            let id = PanelId::parse(id).ok_or_else(|| {
                format!(
                    "unknown panel \"{}\" (known: {})",
                    id.trim(),
                    PanelId::ALL.map(PanelId::as_str).join(", ")
                )
            })?;
            cells.push((Fill(weight), p(id)));
        }
        if cells.is_empty() {
            return Err("a row has no panels".into());
        }
        let node = if cells.len() == 1 {
            cells.remove(0).1
        } else {
            Row(cells)
        };
        let size = match (row.height, row.weight) {
            (Some(h), _) => Len(h),
            (None, w) => Fill(w.unwrap_or(1)),
        };
        rows.push((size, node));
    }
    if rows.is_empty() {
        return Err("no rows".into());
    }
    Ok(LayoutSpec {
        name: user.name.trim().to_string(),
        variants: vec![Variant {
            min: (0, 0),
            root: Col(rows),
        }],
    })
}

/// Where each panel of a layout goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub id: PanelId,
    pub area: Rect,
}

/// Places `spec` in `area` for a `window`-sized terminal.
pub fn solve(
    spec: &LayoutSpec,
    area: Rect,
    window: Rect,
    gaps: u16,
    vm: Option<&ViewModel>,
) -> Vec<Placement> {
    let mut out = Vec::new();
    if let Some(v) = spec.variant(window.width, window.height) {
        place(&v.root, area, gaps, vm, &mut out);
    }
    out
}

/// Renders `spec` in `area`.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    spec: &LayoutSpec,
    vm: &ViewModel,
    theme: &Theme,
    gaps: u16,
) {
    let window = frame.area();
    for placed in solve(spec, area, window, gaps, Some(vm)) {
        render_panel(frame, placed.area, placed.id.panel(), vm, theme);
    }
}

/// Whether a node has a border (and so wants gaps around it).
fn boxed(node: &Node) -> bool {
    match node {
        Node::Panel(id) => id.panel().boxed(),
        Row(children) | Col(children) => children.iter().any(|(_, c)| boxed(c)),
    }
}

/// The most important (lowest) priority in a node.
fn priority(node: &Node) -> u8 {
    match node {
        Node::Panel(id) => id.panel().priority(),
        Row(children) | Col(children) => children
            .iter()
            .map(|(_, c)| priority(c))
            .min()
            .unwrap_or(u8::MAX),
    }
}

/// The least a node needs along `dir` to show at least its summaries.
fn least(node: &Node, dir: Direction, gaps: u16) -> u16 {
    match node {
        Node::Panel(_) => match dir {
            Direction::Vertical => 1,
            Direction::Horizontal => 12,
        },
        Row(children) | Col(children) => {
            let along = matches!(
                (node, dir),
                (Row(_), Direction::Horizontal) | (Col(_), Direction::Vertical)
            );
            let sizes = children.iter().map(|(_, c)| least(c, dir, gaps));
            if along {
                let n = children.len() as u16;
                sizes.sum::<u16>() + gaps * n.saturating_sub(1)
            } else {
                sizes.max().unwrap_or(0)
            }
        }
    }
}

/// What an `auto` child would like.
fn preferred(node: &Node, vm: Option<&ViewModel>) -> Option<u16> {
    let vm = vm?;
    match node {
        Node::Panel(id) => id.panel().preferred_height(vm),
        Row(children) | Col(children) => children
            .iter()
            .filter_map(|(_, c)| preferred(c, Some(vm)))
            .max(),
    }
}

fn place(node: &Node, area: Rect, gaps: u16, vm: Option<&ViewModel>, out: &mut Vec<Placement>) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let (children, dir) = match node {
        Node::Panel(id) => {
            out.push(Placement { id: *id, area });
            return;
        }
        Row(c) => (c, Direction::Horizontal),
        Col(c) => (c, Direction::Vertical),
    };
    let room = match dir {
        Direction::Horizontal => area.width,
        Direction::Vertical => area.height,
    };
    // Drop the least important children until the rest fit as summaries.
    let mut kept: Vec<usize> = (0..children.len()).collect();
    let need = |kept: &[usize]| -> u16 {
        let mut total = 0u16;
        for (n, &i) in kept.iter().enumerate() {
            total = total.saturating_add(least(&children[i].1, dir, gaps));
            if n > 0 && gap_between(children, kept[n - 1], i) {
                total = total.saturating_add(gaps);
            }
        }
        total
    };
    while kept.len() > 1 && need(&kept) > room {
        let worst = kept
            .iter()
            .enumerate()
            .max_by_key(|&(n, &i)| (priority(&children[i].1), n))
            .map(|(n, _)| n);
        match worst {
            Some(n) => {
                kept.remove(n);
            }
            None => break,
        }
    }
    let mut constraints = Vec::new();
    let mut slots = Vec::new();
    for (n, &i) in kept.iter().enumerate() {
        if n > 0 && gap_between(children, kept[n - 1], i) && gaps > 0 {
            constraints.push(Constraint::Length(gaps));
        }
        let (size, child) = &children[i];
        constraints.push(match *size {
            Len(l) => Constraint::Length(l),
            Fill(w) => Constraint::Fill(w.max(1)),
            Pct(pc) => Constraint::Percentage(pc.min(100)),
            Max(m) => Constraint::Max(m),
            Min(m) => Constraint::Min(m),
            Auto(lo, hi) => {
                Constraint::Length(preferred(child, vm).unwrap_or(lo).clamp(lo, hi.max(lo)))
            }
        });
        slots.push((constraints.len() - 1, i));
    }
    let rects = Layout::new(dir, constraints).split(area);
    for (slot, i) in slots {
        if let Some(rect) = rects.get(slot) {
            place(&children[i].1, *rect, gaps, vm, out);
        }
    }
}

/// Whether a child shows a border: boxed and taller than one row.
fn framed(size: Size, node: &Node) -> bool {
    size != Len(1) && boxed(node)
}

/// Gaps go only between two framed neighbours.
fn gap_between(children: &[(Size, Node)], a: usize, b: usize) -> bool {
    match (children.get(a), children.get(b)) {
        (Some((sx, x)), Some((sy, y))) => framed(*sx, x) && framed(*sy, y),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_application::RowSettings;

    fn ids(placements: &[Placement]) -> Vec<&'static str> {
        placements.iter().map(|p| p.id.as_str()).collect()
    }

    fn solve_at(spec: &LayoutSpec, w: u16, h: u16, gaps: u16) -> Vec<Placement> {
        let area = Rect::new(0, 0, w, h);
        solve(spec, area, area, gaps, None)
    }

    #[test]
    fn command_picks_variants_by_window_size() {
        let c = command();
        assert!(ids(&solve_at(&c, 160, 48, 1)).contains(&"who"));
        assert!(!ids(&solve_at(&c, 130, 40, 1)).contains(&"who"));
        let compact = solve_at(&c, 100, 30, 1);
        let reviewer = compact.iter().find(|p| p.id == P::Reviewer).unwrap();
        assert_eq!(reviewer.area.height, 1, "the reviewer is one line");
        assert!(ids(&solve_at(&c, 60, 20, 1)).contains(&"size-hint"));
    }

    #[test]
    fn placements_stay_inside_and_do_not_overlap() {
        for spec in presets() {
            for (w, h) in [(20, 5), (80, 24), (120, 40), (200, 60), (260, 80)] {
                let area = Rect::new(0, 0, w, h);
                let placed = solve(&spec, area, area, 1, None);
                for (i, a) in placed.iter().enumerate() {
                    assert!(area.contains(a.area.as_position()) || a.area.is_empty());
                    assert!(a.area.right() <= w && a.area.bottom() <= h);
                    for b in &placed[i + 1..] {
                        assert!(
                            !a.area.intersects(b.area),
                            "{} {w}x{h}: {a:?} {b:?}",
                            spec.name
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn gaps_separate_boxed_neighbours_only() {
        let spec = LayoutSpec {
            name: "t".into(),
            variants: vec![Variant {
                min: (0, 0),
                root: Col(vec![
                    (Len(1), p(P::Steps)),
                    (Len(1), p(P::Command)),
                    (
                        Fill(1),
                        Row(vec![(Fill(1), p(P::Lead)), (Fill(1), p(P::Guard))]),
                    ),
                ]),
            }],
        };
        let placed = solve_at(&spec, 41, 10, 1);
        let at = |id| placed.iter().find(|p| p.id == id).unwrap().area;
        assert_eq!(at(P::Command).y, 1, "strips touch");
        assert_eq!(at(P::Lead).y, 2);
        assert_eq!(at(P::Guard).x, at(P::Lead).right() + 1, "one blank column");
        let tight = solve_at(&spec, 41, 10, 0);
        let lead = tight.iter().find(|p| p.id == P::Lead).unwrap().area;
        assert_eq!(lead.width, 21);
    }

    #[test]
    fn least_important_panels_go_first() {
        let b = btop();
        let small = ids(&solve_at(&b, 30, 6, 1));
        assert!(
            small.contains(&"steps") || small.contains(&"lead"),
            "{small:?}"
        );
        assert!(!small.contains(&"env"), "{small:?}");
        let big = ids(&solve_at(&b, 200, 60, 1));
        for id in [
            "lead",
            "usage",
            "guard",
            "subagents",
            "log",
            "calendar",
            "env",
            "mix",
        ] {
            assert!(big.contains(&id), "{id} missing: {big:?}");
        }
    }

    #[test]
    fn user_layouts_parse_ids_and_weights() {
        let user = LayoutSettings {
            name: " focus ".into(),
            rows: vec![
                RowSettings {
                    panels: vec!["lead:2".into(), "guard".into()],
                    height: Some(6),
                    weight: None,
                },
                RowSettings {
                    panels: vec!["log".into()],
                    height: None,
                    weight: Some(3),
                },
            ],
        };
        let spec = from_settings(&user).unwrap();
        assert_eq!(spec.name, "focus");
        assert_eq!(
            spec.variants[0].root,
            Col(vec![
                (
                    Len(6),
                    Row(vec![(Fill(2), p(P::Lead)), (Fill(1), p(P::Guard))])
                ),
                (Fill(3), p(P::Log)),
            ])
        );
        let placed = solve_at(&spec, 60, 20, 1);
        let lead = placed.iter().find(|p| p.id == P::Lead).unwrap().area;
        assert_eq!((lead.height, lead.width), (6, 39));

        let bad = |panels: &[&str]| LayoutSettings {
            name: "x".into(),
            rows: vec![RowSettings {
                panels: panels.iter().map(|s| s.to_string()).collect(),
                height: None,
                weight: None,
            }],
        };
        assert!(
            from_settings(&bad(&["nope"]))
                .unwrap_err()
                .contains("unknown panel")
        );
        assert!(
            from_settings(&bad(&["lead:x"]))
                .unwrap_err()
                .contains("weight")
        );
        assert!(from_settings(&bad(&[])).is_err());
    }
}
