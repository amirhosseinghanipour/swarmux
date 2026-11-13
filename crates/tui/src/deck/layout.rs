//! The Deck's graph laid out: which column each card sits in, in what
//! order, at which row, and the lines between them on a grid of cells.
//!
//! Pure: no terminal types, no I/O, no clock. The same graph always gives
//! the same layout, so what is drawn does not shuffle when nothing changed.
//!
//! The method is the layered one (Sugiyama): a card's column is its depth
//! from the read model (producers at the left), a line that crosses
//! columns gets a "wire" slot in each column it passes, the order of the
//! slots in a column is improved by barycentre sweeps (fewer crossings,
//! good enough, not optimal), and rows are pulled toward the neighbours'.
//!
//! What it does with what a graph is not:
//!
//! - **Cycles.** Depths come from the read model, which breaks cycles
//!   before assigning them, so a dependency or contract that runs from a
//!   deeper card to a shallower one is a *back* edge: it is not drawn and is reported in
//!   [`Layered::back`], so the screen can mark it. Nothing loops.
//! - **Isolated cards** (no edge that can be drawn) leave the columns and
//!   go to a shelf below, in rows, so a hundred unrelated jobs do not make
//!   one tall column.
//! - **File claims** have no depth: the card holding the files goes left
//!   of the one waiting for them, unless that would close a cycle with the
//!   edges already taken (then the claim is a back edge, in words only).
//! - **Edges inside a column** (two cards of one depth joined by a
//!   dependency or a contract that the depths do not order) cannot be drawn
//!   between columns: they are *lateral*, listed in [`Layered::lateral`] and
//!   described in words by the screen.
//! - **Two edges between the same cards** are drawn as one line, the most
//!   urgent (blocked, then pending, then met; then dependency before
//!   contract before claim); the others are in [`Layered::merged`].
//! - **More cards than rows.** The canvas is as tall as it needs; the
//!   screen scrolls it so the selected card is always in view
//!   ([`follow`]).
//! - **Too narrow** for the columns ([`fit`] is `None`): the screen draws a
//!   list instead.

use std::collections::BTreeMap;

use swarmux_application::console::{DepState, EdgeKind};

/// What the layout needs to know of a card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InNode {
    pub id: String,
    /// Producers before it: its column.
    pub depth: u32,
    /// Where its outcome sits in the queue: cards of one outcome stay
    /// together within a column.
    pub group: usize,
}

/// An edge by card index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InEdge {
    pub from: usize,
    pub to: usize,
    pub kind: EdgeKind,
    pub state: DepState,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Input {
    pub nodes: Vec<InNode>,
    pub edges: Vec<InEdge>,
}

/// One place in a column: a card, or the row a long edge passes through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Slot {
    Node(usize),
    /// The wire of drawn edge `n` (an index into [`Layered::drawn`]).
    Wire(usize),
}

/// An edge that is drawn, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drawn {
    /// Index into [`Input::edges`].
    pub edge: usize,
    /// The card at its left end.
    pub src: usize,
    /// The card at its right end.
    pub dst: usize,
    pub kind: EdgeKind,
    pub state: DepState,
}

/// Columns, order and what was left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layered {
    /// The column of each card; `None` for a card on the shelf.
    pub column: Vec<Option<usize>>,
    /// The slots of each column, top to bottom.
    pub columns: Vec<Vec<Slot>>,
    /// Cards with no drawn edge, in the order they go on the shelf.
    pub shelf: Vec<usize>,
    pub drawn: Vec<Drawn>,
    /// Edges between two cards of one column (indices into the input's).
    pub lateral: Vec<usize>,
    /// Edges that run against the columns: a cycle that was broken.
    pub back: Vec<usize>,
    /// Edges between cards that another edge already draws.
    pub merged: Vec<usize>,
}

fn kind_rank(k: EdgeKind) -> u8 {
    match k {
        EdgeKind::TaskDependency => 2,
        EdgeKind::ContractLink => 1,
        EdgeKind::FileClaim => 0,
    }
}

fn state_rank(s: DepState) -> u8 {
    match s {
        DepState::Blocked => 2,
        DepState::Pending => 1,
        DepState::Met => 0,
    }
}

/// How urgent an edge is to see: blocked, pending, met, then its kind.
pub fn urgency(kind: EdgeKind, state: DepState) -> (u8, u8) {
    (state_rank(state), kind_rank(kind))
}

/// Sweeps of the crossing reduction at most (down and up count as two).
const SWEEPS: usize = 8;
/// More slots than this and the crossing reduction is skipped: the order
/// by outcome and index stands, and the layout stays fast.
const SWEEP_LIMIT: usize = 3000;

/// Puts the cards in columns and orders them.
pub fn layered(input: &Input) -> Layered {
    let n = input.nodes.len();
    let mut out = Layered {
        column: vec![None; n],
        ..Layered::default()
    };

    // Which edges can be drawn, and from which card to which. Dependencies
    // and contracts go by the read model's depths. A claim has no depth of
    // its own: the card that holds the files goes left of the one that
    // waits, unless that would close a cycle with the edges already taken.
    let mut accepted: Vec<(usize, usize, usize)> = Vec::new(); // (edge, src, dst)
    for (i, e) in input.edges.iter().enumerate() {
        if e.from >= n || e.to >= n || e.from == e.to || e.kind == EdgeKind::FileClaim {
            continue;
        }
        let (df, dt) = (input.nodes[e.from].depth, input.nodes[e.to].depth);
        match df.cmp(&dt) {
            std::cmp::Ordering::Less => accepted.push((i, e.from, e.to)),
            std::cmp::Ordering::Equal => out.lateral.push(i),
            std::cmp::Ordering::Greater => out.back.push(i),
        }
    }
    for (i, e) in input.edges.iter().enumerate() {
        if e.from >= n || e.to >= n || e.from == e.to || e.kind != EdgeKind::FileClaim {
            continue;
        }
        if reaches(&accepted, e.to, e.from) {
            out.back.push(i);
        } else {
            accepted.push((i, e.from, e.to));
        }
    }
    out.back.sort_unstable();
    out.lateral.sort_unstable();

    // Two edges between the same cards are one line: the most urgent.
    let mut candidates: BTreeMap<(usize, usize), Drawn> = BTreeMap::new();
    for (i, src, dst) in accepted {
        let e = input.edges[i];
        let drawn = Drawn {
            edge: i,
            src,
            dst,
            kind: e.kind,
            state: e.state,
        };
        match candidates.get(&(src, dst)) {
            Some(held) if urgency(held.kind, held.state) >= urgency(e.kind, e.state) => {
                out.merged.push(i);
            }
            Some(held) => {
                out.merged.push(held.edge);
                candidates.insert((src, dst), drawn);
            }
            None => {
                candidates.insert((src, dst), drawn);
            }
        }
    }
    out.merged.sort_unstable();
    out.drawn = candidates.into_values().collect();
    out.drawn.sort_by_key(|d| (d.src, d.dst));

    // Cards with a drawn edge go in columns; the others to the shelf.
    let mut linked = vec![false; n];
    for d in &out.drawn {
        linked[d.src] = true;
        linked[d.dst] = true;
    }
    let by_place =
        |a: &usize, b: &usize| (input.nodes[*a].group, *a).cmp(&(input.nodes[*b].group, *b));
    out.shelf = (0..n).filter(|i| !linked[*i]).collect();
    out.shelf.sort_by(by_place);

    // A card's column is the longest chain of drawn edges before it.
    let level = levels(n, &out.drawn);
    let mut depths: Vec<usize> = (0..n).filter(|i| linked[*i]).map(|i| level[i]).collect();
    depths.sort_unstable();
    depths.dedup();
    let count = depths.len();
    for i in (0..n).filter(|i| linked[*i]) {
        out.column[i] = depths.binary_search(&level[i]).ok();
    }
    if count == 0 {
        return out;
    }

    // Slots of every column, in a first order: by outcome, then index.
    let mut columns: Vec<Vec<Slot>> = vec![Vec::new(); count];
    let mut linked_nodes: Vec<usize> = (0..n).filter(|i| linked[*i]).collect();
    linked_nodes.sort_by(by_place);
    for i in linked_nodes {
        if let Some(c) = out.column[i] {
            columns[c].push(Slot::Node(i));
        }
    }
    for (k, d) in out.drawn.iter().enumerate() {
        let (a, b) = (col(&out, d.src), col(&out, d.dst));
        for column in &mut columns[a + 1..b] {
            column.push(Slot::Wire(k));
        }
    }
    let hops = hops_of(&out, count);
    let slots: usize = columns.iter().map(Vec::len).sum();
    if slots <= SWEEP_LIMIT {
        reduce_crossings(&mut columns, &hops);
    }
    out.columns = columns;
    out
}

/// Whether `target` can be reached from `start` along `edges` (src to dst).
fn reaches(edges: &[(usize, usize, usize)], start: usize, target: usize) -> bool {
    let mut next: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (_, a, b) in edges {
        next.entry(*a).or_default().push(*b);
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut stack = vec![start];
    while let Some(at) = stack.pop() {
        if at == target {
            return true;
        }
        if seen.insert(at) {
            stack.extend(next.get(&at).into_iter().flatten().copied());
        }
    }
    false
}

/// The longest chain of drawn edges ending at each card. The edges form a
/// DAG by construction, so this ends.
fn levels(n: usize, drawn: &[Drawn]) -> Vec<usize> {
    let mut level = vec![0usize; n];
    let mut indegree = vec![0usize; n];
    let mut next: Vec<Vec<usize>> = vec![Vec::new(); n];
    for d in drawn {
        next[d.src].push(d.dst);
        indegree[d.dst] += 1;
    }
    let mut ready: Vec<usize> = (0..n).filter(|i| indegree[*i] == 0).collect();
    while let Some(at) = ready.pop() {
        for to in next[at].clone() {
            level[to] = level[to].max(level[at] + 1);
            indegree[to] -= 1;
            if indegree[to] == 0 {
                ready.push(to);
            }
        }
    }
    level
}

fn col(l: &Layered, node: usize) -> usize {
    l.column[node].unwrap_or(0)
}

/// For each gap between column `c` and `c + 1`, the slots a drawn edge
/// joins across it: (slot at `c`, slot at `c + 1`).
fn hops_of(l: &Layered, count: usize) -> Vec<Vec<(Slot, Slot)>> {
    let mut hops = vec![Vec::new(); count.saturating_sub(1)];
    for (k, d) in l.drawn.iter().enumerate() {
        let (a, b) = (col(l, d.src), col(l, d.dst));
        for (c, gap) in hops.iter_mut().enumerate().take(b).skip(a) {
            let from = if c == a {
                Slot::Node(d.src)
            } else {
                Slot::Wire(k)
            };
            let to = if c + 1 == b {
                Slot::Node(d.dst)
            } else {
                Slot::Wire(k)
            };
            gap.push((from, to));
        }
    }
    hops
}

fn positions(column: &[Slot]) -> BTreeMap<Slot, usize> {
    column.iter().enumerate().map(|(i, s)| (*s, i)).collect()
}

/// Pairs of hops across one gap that cross.
fn crossings(columns: &[Vec<Slot>], hops: &[Vec<(Slot, Slot)>]) -> usize {
    let mut total = 0;
    for (c, gap) in hops.iter().enumerate() {
        let (left, right) = (positions(&columns[c]), positions(&columns[c + 1]));
        let at: Vec<(usize, usize)> = gap.iter().map(|(a, b)| (left[a], right[b])).collect();
        for (i, x) in at.iter().enumerate() {
            for y in &at[i + 1..] {
                if (x.0 < y.0 && x.1 > y.1) || (x.0 > y.0 && x.1 < y.1) {
                    total += 1;
                }
            }
        }
    }
    total
}

/// Barycentre sweeps down and up the columns, keeping the best order seen.
fn reduce_crossings(columns: &mut [Vec<Slot>], hops: &[Vec<(Slot, Slot)>]) {
    if columns.len() < 2 {
        return;
    }
    let mut best = columns.to_vec();
    let mut best_score = crossings(columns, hops);
    for sweep in 0..SWEEPS {
        if best_score == 0 {
            break;
        }
        let down = sweep % 2 == 0;
        let order: Vec<usize> = if down {
            (1..columns.len()).collect()
        } else {
            (0..columns.len() - 1).rev().collect()
        };
        for c in order {
            let reference = if down { c - 1 } else { c + 1 };
            let there = positions(&columns[reference]);
            let gap = if down { &hops[c - 1] } else { &hops[c] };
            let mut keyed: Vec<(f64, usize, Slot)> = columns[c]
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let near: Vec<usize> = gap
                        .iter()
                        .filter_map(|(a, b)| {
                            let (mine, other) = if down { (b, a) } else { (a, b) };
                            (mine == s).then(|| there[other])
                        })
                        .collect();
                    let value = if near.is_empty() {
                        i as f64
                    } else {
                        near.iter().sum::<usize>() as f64 / near.len() as f64
                    };
                    (value, i, *s)
                })
                .collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            columns[c] = keyed.into_iter().map(|k| k.2).collect();
        }
        let score = crossings(columns, hops);
        if score < best_score {
            best_score = score;
            best = columns.to_vec();
        }
    }
    columns.clone_from_slice(&best);
}

/// How big the cards and the gaps between columns are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    pub card_w: u16,
    pub card_h: u16,
    /// Cells between two columns, for the lines.
    pub gutter: u16,
    /// Empty rows between two slots of a column.
    pub gap: u16,
}

/// The widest a card gets, and the narrowest the graph accepts.
pub const CARD_MAX: u16 = 36;
pub const CARD_MIN: u16 = 26;
/// Rows of a card: border, state line, checks line, title line, border.
pub const CARD_H: u16 = 5;

/// The sizes for `columns` columns in `width` cells; `None` when they do
/// not fit and the screen should draw a list instead.
pub fn fit(columns: usize, width: u16) -> Option<Metrics> {
    let columns = u16::try_from(columns.max(1)).ok()?;
    for gutter in [8u16, 6, 5] {
        let gaps = gutter.checked_mul(columns - 1)?;
        let room = width.checked_sub(gaps)? / columns;
        if room >= CARD_MIN {
            return Some(Metrics {
                card_w: room.min(CARD_MAX),
                card_h: CARD_H,
                gutter,
                gap: 1,
            });
        }
    }
    None
}

/// Where a card goes, in cells from the canvas's top left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pos {
    pub x: u16,
    pub y: u16,
}

// Direction bits of a cell of the canvas.
const N: u8 = 1;
const E: u8 = 2;
const S: u8 = 4;
const W: u8 = 8;

/// One cell of the lines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cell {
    bits: u8,
    /// The edge that decided how it looks: (kind, state, touches the
    /// selected card).
    look: Option<(EdgeKind, DepState, bool)>,
    /// An arrowhead into a card.
    head: bool,
}

impl Cell {
    pub fn look(&self) -> Option<(EdgeKind, DepState, bool)> {
        self.look
    }

    /// The character: box drawing for the lines (dotted for a contract and
    /// a claim), an arrow for a head.
    pub fn glyph(&self) -> char {
        if self.head {
            return '▶';
        }
        let kind = self.look.map(|l| l.0);
        let dotted = |solid: char, contract: char, claim: char| match kind {
            Some(EdgeKind::ContractLink) => contract,
            Some(EdgeKind::FileClaim) => claim,
            _ => solid,
        };
        match self.bits {
            0 => ' ',
            b if b & (E | W) == b => dotted('─', '┈', '╌'),
            b if b & (N | S) == b => dotted('│', '┊', '╎'),
            b if b == S | E => '┌',
            b if b == S | W => '┐',
            b if b == N | E => '└',
            b if b == N | W => '┘',
            b if b == N | S | E => '├',
            b if b == N | S | W => '┤',
            b if b == E | W | S => '┬',
            b if b == E | W | N => '┴',
            _ => '┼',
        }
    }
}

/// The lines, a cell each.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Canvas {
    pub width: u16,
    pub height: u16,
    cells: Vec<Cell>,
}

impl Canvas {
    fn new(width: u16, height: u16) -> Canvas {
        Canvas {
            width,
            height,
            cells: vec![Cell::default(); usize::from(width) * usize::from(height)],
        }
    }

    pub fn cell(&self, x: u16, y: u16) -> Cell {
        if x < self.width && y < self.height {
            self.cells[usize::from(y) * usize::from(self.width) + usize::from(x)]
        } else {
            Cell::default()
        }
    }

    fn put(&mut self, x: u16, y: u16, bits: u8, look: (EdgeKind, DepState, bool), head: bool) {
        if x >= self.width || y >= self.height {
            return;
        }
        let w = usize::from(self.width);
        let cell = &mut self.cells[usize::from(y) * w + usize::from(x)];
        cell.bits |= bits;
        cell.head |= head;
        let rank = |l: (EdgeKind, DepState, bool)| (l.2, state_rank(l.1), kind_rank(l.0));
        if cell.look.is_none_or(|old| rank(look) > rank(old)) {
            cell.look = Some(look);
        }
    }

    fn horizontal(&mut self, y: u16, x0: u16, x1: u16, look: (EdgeKind, DepState, bool)) {
        for x in x0..=x1 {
            self.put(x, y, E | W, look, false);
        }
    }
}

/// Where everything is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Geometry {
    pub metrics: Option<Metrics>,
    /// Each card's top left; `None` before it is placed.
    pub cards: Vec<Option<Pos>>,
    pub canvas: Canvas,
    /// The row where the shelf's heading goes, when there is a shelf, and
    /// the cell where it starts.
    pub shelf_top: Option<u16>,
    pub shelf_x: u16,
    pub width: u16,
    pub height: u16,
}

impl Geometry {
    /// Rows a card takes: `(top, bottom)` exclusive, when placed.
    pub fn rows_of(&self, node: usize) -> Option<(u16, u16)> {
        let m = self.metrics?;
        let p = self.cards.get(node).copied().flatten()?;
        Some((p.y, p.y + m.card_h))
    }
}

fn height_of(slot: Slot, m: &Metrics) -> u16 {
    match slot {
        Slot::Node(_) => m.card_h,
        Slot::Wire(_) => 1,
    }
}

/// The rows nearest to `want` that keep every slot below the one before
/// it with `gap` rows between, and none above row 0: the least-squares fit
/// under that order (pool adjacent violators), so rows do not drift down
/// from pass to pass.
fn settle(want: &[i32], heights: &[i32], gap: i32) -> Vec<i32> {
    let n = want.len();
    let mut offset = vec![0i32; n];
    for k in 1..n {
        offset[k] = offset[k - 1] + heights[k - 1] + gap;
    }
    // Blocks of slots that move together: (sum of targets, count).
    let mut blocks: Vec<(i64, i64)> = Vec::new();
    for k in 0..n {
        blocks.push((i64::from(want[k] - offset[k]), 1));
        while blocks.len() >= 2 {
            let (a, b) = (blocks[blocks.len() - 2], blocks[blocks.len() - 1]);
            // The earlier block's mean must not exceed the later one's.
            if a.0.div_euclid(a.1) <= b.0.div_euclid(b.1) {
                break;
            }
            blocks.truncate(blocks.len() - 2);
            blocks.push((a.0 + b.0, a.1 + b.1));
        }
    }
    let mut tops = Vec::with_capacity(n);
    for (sum, count) in blocks {
        let mean = i32::try_from(sum.div_euclid(count)).unwrap_or(0);
        for _ in 0..count {
            let k = tops.len();
            tops.push(mean + offset[k]);
        }
    }
    let low = tops.iter().copied().min().unwrap_or(0);
    if low < 0 {
        for t in &mut tops {
            *t -= low;
        }
    }
    tops
}

/// Rows each slot's top sits at, pulled toward its neighbours'.
fn rows(l: &Layered, m: &Metrics) -> Vec<Vec<i32>> {
    let gap = i32::from(m.gap);
    let mut tops: Vec<Vec<i32>> = Vec::new();
    for column in &l.columns {
        let mut y = 0i32;
        let mut col = Vec::new();
        for s in column {
            col.push(y);
            y += i32::from(height_of(*s, m)) + gap;
        }
        tops.push(col);
    }
    if l.columns.len() < 2 {
        return tops;
    }
    let hops = hops_of(l, l.columns.len());
    let index: Vec<BTreeMap<Slot, usize>> = l.columns.iter().map(|c| positions(c)).collect();
    for round in 0..8 {
        let forward = round % 2 == 0;
        let order: Vec<usize> = if forward {
            (1..l.columns.len()).collect()
        } else {
            (0..l.columns.len() - 1).rev().collect()
        };
        for c in order {
            let reference = if forward { c - 1 } else { c + 1 };
            let gap_hops = if forward { &hops[c - 1] } else { &hops[c] };
            let mut want: Vec<i32> = Vec::new();
            for (k, s) in l.columns[c].iter().enumerate() {
                let centres: Vec<i32> = gap_hops
                    .iter()
                    .filter_map(|(a, b)| {
                        let (mine, other) = if forward { (b, a) } else { (a, b) };
                        (mine == s).then(|| {
                            let at = index[reference][other];
                            tops[reference][at]
                                + i32::from(height_of(l.columns[reference][at], m)) / 2
                        })
                    })
                    .collect();
                let own = i32::from(height_of(*s, m)) / 2;
                want.push(if centres.is_empty() {
                    tops[c][k]
                } else {
                    centres.iter().sum::<i32>() / centres.len() as i32 - own
                });
            }
            let heights: Vec<i32> = l.columns[c]
                .iter()
                .map(|s| i32::from(height_of(*s, m)))
                .collect();
            tops[c] = settle(&want, &heights, gap);
        }
    }
    let low = tops.iter().flatten().copied().min().unwrap_or(0);
    for t in tops.iter_mut().flatten() {
        *t -= low;
    }
    tops
}

/// Places every card and draws the lines. `selected` is the card whose
/// lines are marked.
pub fn geometry(
    input: &Input,
    l: &Layered,
    m: Metrics,
    width: u16,
    selected: Option<usize>,
) -> Geometry {
    let n = input.nodes.len();
    let mut cards: Vec<Option<Pos>> = vec![None; n];
    let tops = rows(l, &m);
    let cols = l.columns.len();
    let pitch = m.card_w + m.gutter;
    let mut height = 0u16;
    let mut centres: BTreeMap<(usize, Slot), u16> = BTreeMap::new();
    for (c, column) in l.columns.iter().enumerate() {
        for (k, s) in column.iter().enumerate() {
            let y = u16::try_from(tops[c][k]).unwrap_or(0);
            let h = height_of(*s, &m);
            height = height.max(y + h);
            centres.insert((c, *s), y + h / 2);
            if let Slot::Node(i) = s {
                cards[*i] = Some(Pos {
                    x: u16::try_from(c).unwrap_or(0) * pitch,
                    y,
                });
            }
        }
    }
    let graph_w = if cols == 0 {
        0
    } else {
        u16::try_from(cols).unwrap_or(1) * pitch - m.gutter
    };

    // The shelf: rows of cards, beside the columns when the width leaves
    // room and that is not taller than putting them underneath.
    let mut shelf_top = None;
    let mut shelf_x = 0u16;
    if !l.shelf.is_empty() {
        let n_shelf = l.shelf.len();
        let slot_w = m.card_w + 2;
        let row_h = m.card_h + m.gap;
        let rows_for = |per_row: usize| u16::try_from(n_shelf.div_ceil(per_row)).unwrap_or(1);
        let below_per_row = usize::from(((width + 2) / slot_w).max(1));
        let below_top = if cols == 0 { 0 } else { height + 1 };
        let below_total = below_top + 1 + rows_for(below_per_row) * row_h - m.gap;
        let x0 = graph_w + 4;
        let beside = (cols > 0 && width >= x0 + m.card_w).then(|| {
            let per_row = usize::from(((width - x0 + 2) / slot_w).max(1));
            (per_row, height.max(1 + rows_for(per_row) * row_h - m.gap))
        });
        let (per_row, top, x, total) = match beside {
            Some((per_row, total)) if total <= below_total => (per_row, 0, x0, total),
            _ => (below_per_row, below_top, 0, below_total),
        };
        shelf_top = Some(top);
        shelf_x = x;
        for (k, node) in l.shelf.iter().enumerate() {
            let (row, at) = (k / per_row, k % per_row);
            cards[*node] = Some(Pos {
                x: x + u16::try_from(at).unwrap_or(0) * slot_w,
                y: top + 1 + u16::try_from(row).unwrap_or(0) * row_h,
            });
        }
        height = total;
    }
    let total_w = graph_w.max(if l.shelf.is_empty() { 0 } else { width });
    let mut canvas = Canvas::new(total_w.max(1), height.max(1));

    // The lines, gap by gap, each in a lane of its own.
    for c in 0..cols.saturating_sub(1) {
        let mut hops: Vec<(usize, i32, i32)> = Vec::new(); // (drawn, from row, to row)
        for (k, d) in l.drawn.iter().enumerate() {
            let (a, b) = (col(l, d.src), col(l, d.dst));
            if !(a..b).contains(&c) {
                continue;
            }
            let from = if c == a {
                Slot::Node(d.src)
            } else {
                Slot::Wire(k)
            };
            let to = if c + 1 == b {
                Slot::Node(d.dst)
            } else {
                Slot::Wire(k)
            };
            hops.push((
                k,
                i32::from(centres[&(c, from)]),
                i32::from(centres[&(c + 1, to)]),
            ));
        }
        let mut down: Vec<_> = hops.iter().filter(|h| h.2 > h.1).copied().collect();
        let mut up: Vec<_> = hops.iter().filter(|h| h.2 < h.1).copied().collect();
        // Lower sources take the lanes at the left going down, upper ones
        // going up, so the lines cross as little as the order allows.
        down.sort_by_key(|h| (std::cmp::Reverse(h.1), h.0));
        up.sort_by_key(|h| (h.1, h.0));
        let lanes = usize::from(m.gutter.saturating_sub(2)).max(1);
        let lane_of: BTreeMap<usize, usize> = down
            .iter()
            .chain(up.iter())
            .enumerate()
            .map(|(i, h)| (h.0, i.min(lanes - 1)))
            .collect();

        let x0 = u16::try_from(c).unwrap_or(0) * pitch + m.card_w;
        let x1 = x0 + m.gutter - 1;
        for (k, from, to) in hops {
            let d = l.drawn[k];
            let hot = selected.is_some_and(|s| s == d.src || s == d.dst);
            let look = (d.kind, d.state, hot);
            let (yf, yt) = (
                u16::try_from(from).unwrap_or(0),
                u16::try_from(to).unwrap_or(0),
            );
            let into_card = c + 1 == col(l, d.dst);
            if yf == yt {
                canvas.horizontal(yf, x0, x1, look);
            } else {
                let lane = x0 + 1 + u16::try_from(lane_of[&k]).unwrap_or(0);
                canvas.horizontal(yf, x0, lane - 1, look);
                canvas.horizontal(yt, lane + 1, x1, look);
                let (dir_from, dir_to, top, bottom) = if yt > yf {
                    (S, N, yf, yt)
                } else {
                    (N, S, yt, yf)
                };
                canvas.put(lane, yf, W | dir_from, look, false);
                canvas.put(lane, yt, E | dir_to, look, false);
                for y in top + 1..bottom {
                    canvas.put(lane, y, N | S, look, false);
                }
            }
            if into_card {
                canvas.put(x1, yt, E | W, look, true);
            }
            // A wire passes through the column it sits in.
            if c + 1 < col(l, d.dst) {
                let at = u16::try_from(c + 1).unwrap_or(0) * pitch;
                canvas.horizontal(yt, at, at + m.card_w - 1, look);
            }
        }
    }
    Geometry {
        metrics: Some(m),
        cards,
        canvas,
        shelf_top,
        shelf_x,
        width: total_w,
        height,
    }
}

/// The scroll offset that keeps rows `top..bottom` inside a view of `view`
/// rows, moving `offset` as little as it can.
pub fn follow(offset: u16, top: u16, bottom: u16, view: u16) -> u16 {
    if view == 0 {
        return 0;
    }
    let height = bottom.saturating_sub(top);
    if height >= view || top < offset {
        return top;
    }
    if bottom > offset + view {
        return bottom - view;
    }
    offset
}

/// The order `j` and `k` walk: column by column, top to bottom, then the
/// shelf.
pub fn reading_order(l: &Layered) -> Vec<usize> {
    let mut order: Vec<usize> = l
        .columns
        .iter()
        .flatten()
        .filter_map(|s| match s {
            Slot::Node(i) => Some(*i),
            Slot::Wire(_) => None,
        })
        .collect();
    order.extend(&l.shelf);
    order
}

/// The cards `node` is joined to by a drawn edge: the ones that come
/// before it (producers) and after it (consumers), in column order.
pub fn neighbours(l: &Layered, node: usize) -> (Vec<usize>, Vec<usize>) {
    let mut before = Vec::new();
    let mut after = Vec::new();
    for d in &l.drawn {
        if d.dst == node {
            before.push(d.src);
        }
        if d.src == node {
            after.push(d.dst);
        }
    }
    let rank = |i: &usize| {
        l.columns
            .iter()
            .enumerate()
            .find_map(|(c, col)| {
                col.iter()
                    .position(|s| *s == Slot::Node(*i))
                    .map(|p| (c, p))
            })
            .unwrap_or_default()
    };
    before.sort_by_key(rank);
    after.sort_by_key(rank);
    (before, after)
}

#[cfg(test)]
mod tests;
