use super::*;

fn node(id: &str, depth: u32) -> InNode {
    InNode {
        id: id.into(),
        depth,
        group: 0,
    }
}

fn dep(from: usize, to: usize) -> InEdge {
    InEdge {
        from,
        to,
        kind: EdgeKind::TaskDependency,
        state: DepState::Pending,
    }
}

fn input(depths: &[u32], edges: &[(usize, usize)]) -> Input {
    Input {
        nodes: depths
            .iter()
            .enumerate()
            .map(|(i, d)| node(&format!("n{i}"), *d))
            .collect(),
        edges: edges.iter().map(|(a, b)| dep(*a, *b)).collect(),
    }
}

fn place(i: &Input, width: u16) -> (Layered, Geometry) {
    let l = layered(i);
    let m = fit(l.columns.len().max(1), width).expect("fits");
    let g = geometry(i, &l, m, width, None);
    (l, g)
}

/// No two placed cards share a cell.
fn assert_no_overlap(g: &Geometry) {
    let m = g.metrics.unwrap();
    let placed: Vec<(usize, Pos)> = g
        .cards
        .iter()
        .enumerate()
        .filter_map(|(i, p)| p.map(|p| (i, p)))
        .collect();
    for (a, p) in &placed {
        for (b, q) in &placed {
            if a >= b {
                continue;
            }
            let apart = p.x + m.card_w <= q.x
                || q.x + m.card_w <= p.x
                || p.y + m.card_h <= q.y
                || q.y + m.card_h <= p.y;
            assert!(apart, "cards {a} and {b} overlap: {p:?} {q:?}");
        }
    }
}

#[test]
fn a_chain_runs_left_to_right_one_card_a_column() {
    let i = input(&[0, 1, 2], &[(0, 1), (1, 2)]);
    let (l, g) = place(&i, 120);
    assert_eq!(l.column, [Some(0), Some(1), Some(2)]);
    assert_eq!(l.columns.len(), 3);
    let xs: Vec<u16> = g.cards.iter().map(|p| p.unwrap().x).collect();
    assert!(xs[0] < xs[1] && xs[1] < xs[2], "{xs:?}");
    // Each edge ends in an arrowhead just before the card it feeds.
    let m = g.metrics.unwrap();
    for to in [1usize, 2] {
        let p = g.cards[to].unwrap();
        let head = g.canvas.cell(p.x - 1, p.y + m.card_h / 2);
        assert_eq!(head.glyph(), '▶', "into card {to}");
    }
    assert_no_overlap(&g);
}

#[test]
fn the_layout_is_the_same_every_time() {
    let i = input(
        &[0, 0, 1, 1, 2, 3, 2],
        &[(0, 2), (1, 3), (2, 4), (3, 4), (0, 5), (4, 5), (1, 6)],
    );
    assert_eq!(layered(&i), layered(&i));
    let (_, a) = place(&i, 180);
    let (_, b) = place(&i, 180);
    assert_eq!(a, b);
}

#[test]
fn columns_close_up_when_a_depth_has_no_card() {
    let i = input(&[0, 5], &[(0, 1)]);
    let l = layered(&i);
    assert_eq!(l.columns.len(), 2, "depths 0 and 5 are two columns");
    assert_eq!(l.column, [Some(0), Some(1)]);
}

#[test]
fn an_edge_over_a_column_passes_through_a_wire_in_it() {
    // 0 -> 1 -> 2 and 0 -> 2: the long edge crosses the middle column.
    let i = input(&[0, 1, 2], &[(0, 1), (1, 2), (0, 2)]);
    let (l, g) = place(&i, 120);
    let wires = l.columns[1]
        .iter()
        .filter(|s| matches!(s, Slot::Wire(_)))
        .count();
    assert_eq!(wires, 1);
    assert_eq!(l.columns[1].len(), 2, "the card and the wire");
    // The wire row is a line across the whole column, clear of the card.
    let m = g.metrics.unwrap();
    let pitch = m.card_w + m.gutter;
    let card = g.cards[1].unwrap();
    let row = (0..g.canvas.height).find(|y| {
        !(card.y..card.y + m.card_h).contains(y)
            && (pitch..pitch + m.card_w).all(|x| g.canvas.cell(x, *y).glyph() != ' ')
    });
    assert!(
        row.is_some(),
        "a row where the line crosses the middle column"
    );
    assert_no_overlap(&g);
}

#[test]
fn a_cycle_is_broken_marked_and_never_hangs() {
    // a -> b -> c and c -> a: the read model gives depths 0, 1, 2, so c -> a
    // runs against the columns.
    let i = input(&[0, 1, 2], &[(0, 1), (1, 2), (2, 0)]);
    let l = layered(&i);
    assert_eq!(l.back, [2], "the closing edge is the back edge");
    assert_eq!(l.drawn.len(), 2);
    let (_, g) = place(&i, 120);
    assert_no_overlap(&g);
}

#[test]
fn a_cycle_with_no_depth_to_tell_them_apart_is_lateral_not_a_hang() {
    // All at depth 0: every edge is inside one column.
    let i = input(&[0, 0, 0], &[(0, 1), (1, 2), (2, 0)]);
    let l = layered(&i);
    assert_eq!(l.lateral, [0, 1, 2]);
    assert!(l.drawn.is_empty());
    assert_eq!(
        l.shelf,
        [0, 1, 2],
        "with nothing to draw they go to the shelf"
    );
}

#[test]
fn self_loops_and_unknown_cards_are_ignored() {
    let mut i = input(&[0, 1], &[(0, 1)]);
    i.edges.push(dep(1, 1));
    i.edges.push(dep(0, 9));
    let l = layered(&i);
    assert_eq!(l.drawn.len(), 1);
    assert!(l.back.is_empty() && l.lateral.is_empty());
}

#[test]
fn isolated_cards_go_to_a_shelf_in_rows() {
    let i = input(&[0; 30], &[]);
    let (l, g) = place(&i, 180);
    assert!(l.columns.is_empty());
    assert_eq!(l.shelf.len(), 30);
    assert_no_overlap(&g);
    assert!(g.cards.iter().all(Option::is_some), "every card is placed");
    // 180 wide holds four cards a row: eight rows, not one column of thirty.
    let m = g.metrics.unwrap();
    assert!(g.height <= 9 * (m.card_h + m.gap), "{}", g.height);
    assert!(g.height < 30 * (m.card_h + m.gap) / 2);
    assert!(g.cards.iter().flatten().all(|p| p.x + m.card_w <= 180));
}

#[test]
fn the_shelf_sits_below_the_columns_when_there_is_no_room_beside_them() {
    // Two columns fill 60 cells: nothing is left at the right.
    let i = input(&[0, 1, 0], &[(0, 1)]);
    let (l, g) = place(&i, 60);
    assert_eq!(l.shelf, [2]);
    let top = g.shelf_top.expect("a shelf heading");
    assert_eq!(g.shelf_x, 0);
    let linked_bottom = (0..2).map(|n| g.rows_of(n).unwrap().1).max().unwrap();
    assert!(top >= linked_bottom);
    assert!(g.rows_of(2).unwrap().0 > top);
    assert_no_overlap(&g);
}

#[test]
fn the_shelf_sits_beside_the_columns_when_that_is_not_taller() {
    let i = input(&[0, 1, 0, 0], &[(0, 1)]);
    let (l, g) = place(&i, 126);
    assert_eq!(l.shelf, [2, 3]);
    let m = g.metrics.unwrap();
    assert!(g.shelf_x >= 2 * m.card_w, "right of the two columns");
    assert_eq!(g.shelf_top, Some(0));
    for n in [2, 3] {
        assert!(g.cards[n].unwrap().x >= g.shelf_x);
    }
    assert_no_overlap(&g);
}

#[test]
fn two_edges_between_the_same_cards_are_one_line_the_most_urgent() {
    let mut i = input(&[0, 1], &[]);
    i.edges.push(InEdge {
        from: 0,
        to: 1,
        kind: EdgeKind::FileClaim,
        state: DepState::Pending,
    });
    i.edges.push(InEdge {
        from: 0,
        to: 1,
        kind: EdgeKind::TaskDependency,
        state: DepState::Blocked,
    });
    i.edges.push(InEdge {
        from: 0,
        to: 1,
        kind: EdgeKind::ContractLink,
        state: DepState::Met,
    });
    let l = layered(&i);
    assert_eq!(l.drawn.len(), 1);
    assert_eq!(l.drawn[0].edge, 1, "the blocked dependency wins");
    assert_eq!(l.merged, [0, 2]);
}

#[test]
fn a_claim_puts_the_holder_left_of_the_waiter_even_in_one_depth() {
    // Two cards of depth 0: card 0 holds files card 1 waits for.
    let mut i = input(&[0, 0], &[]);
    i.edges.push(InEdge {
        from: 0,
        to: 1,
        kind: EdgeKind::FileClaim,
        state: DepState::Blocked,
    });
    let l = layered(&i);
    assert_eq!((l.drawn[0].src, l.drawn[0].dst), (0, 1));
    assert_eq!(l.column, [Some(0), Some(1)]);
    assert!(l.lateral.is_empty() && l.back.is_empty());
    // And the other way round when the holder is the other card.
    i.edges[0].from = 1;
    i.edges[0].to = 0;
    let l = layered(&i);
    assert_eq!(l.column, [Some(1), Some(0)]);
}

#[test]
fn a_claim_that_would_close_a_cycle_is_left_to_words() {
    // 0 feeds 1 (a dependency); a claim says 1 holds what 0 waits for.
    let mut i = input(&[0, 1], &[(0, 1)]);
    i.edges.push(InEdge {
        from: 1,
        to: 0,
        kind: EdgeKind::FileClaim,
        state: DepState::Blocked,
    });
    let l = layered(&i);
    assert_eq!(l.drawn.len(), 1);
    assert_eq!(l.back, [1]);
    // A dependency the wrong way round is a back edge too.
    let mut j = input(&[0, 1], &[(1, 0)]);
    j.edges[0].kind = EdgeKind::ContractLink;
    assert_eq!(layered(&j).back, [0]);
}

#[test]
fn claims_that_chain_push_the_waiters_right_and_never_loop() {
    // 0 holds for 1, 1 for 2, 2 for 0 (a ring of claims): the third closes
    // the ring and is left out.
    let mut i = input(&[0, 0, 0], &[]);
    for (f, t) in [(0, 1), (1, 2), (2, 0)] {
        i.edges.push(InEdge {
            from: f,
            to: t,
            kind: EdgeKind::FileClaim,
            state: DepState::Pending,
        });
    }
    let l = layered(&i);
    assert_eq!(l.column, [Some(0), Some(1), Some(2)]);
    assert_eq!(l.back, [2]);
}

#[test]
fn the_sweeps_untangle_two_crossing_pairs() {
    // First order: 0 above 1 in column 0, but 2 (fed by 1) above 3 (fed by
    // 0) in column 1: the lines cross. Swapping a column removes it.
    let i = input(&[0, 0, 1, 1], &[(0, 3), (1, 2)]);
    let l = layered(&i);
    let hops = hops_of(&l, l.columns.len());
    assert_eq!(crossings(&l.columns, &hops), 0);
    // The two lines run straight across: same row at both ends.
    let (_, g) = place(&i, 120);
    let m = g.metrics.unwrap();
    for d in &l.drawn {
        let (a, b) = (g.cards[d.src].unwrap(), g.cards[d.dst].unwrap());
        assert_eq!(a.y + m.card_h / 2, b.y + m.card_h / 2, "{d:?} is straight");
    }
}

#[test]
fn a_cards_row_follows_its_neighbours() {
    // One producer feeding the third of three consumers... of one column:
    // it sits level with its consumer, not at the top.
    let i = input(&[0, 1, 1, 1], &[(0, 3)]);
    let (_, g) = place(&i, 120);
    let (p, c) = (g.cards[0].unwrap(), g.cards[3].unwrap());
    assert_eq!(p.y, c.y);
}

#[test]
fn outcomes_stay_together_in_a_column_until_the_edges_say_otherwise() {
    let mut i = input(&[0, 0, 0, 0], &[]);
    for (k, g) in [1, 0, 1, 0].into_iter().enumerate() {
        i.nodes[k].group = g;
    }
    let l = layered(&i);
    assert_eq!(l.shelf, [1, 3, 0, 2], "by outcome, then by index");
}

#[test]
fn wide_graphs_fit_or_fall_back_to_a_list() {
    // The design's size: three columns in the graph pane of 180 columns.
    let m = fit(3, 126).expect("three columns fit 126 cells");
    assert!(m.card_w >= CARD_MIN && m.card_w <= CARD_MAX);
    assert_eq!(fit(1, 100).unwrap().card_w, CARD_MAX);
    // At 120x36 the graph pane is about 80 cells: three columns do not fit.
    assert_eq!(fit(3, 80), None);
    assert!(fit(2, 80).is_some());
    assert_eq!(fit(2, 50), None);
    assert_eq!(fit(1, 10), None);
    // More columns than any screen.
    assert_eq!(fit(40, 250), None);
    assert!(fit(0, 100).is_some(), "no columns is one for the shelf");
    // Whatever fits, the columns and gutters add up to the width.
    for cols in 1..8usize {
        for width in 20..260u16 {
            if let Some(m) = fit(cols, width) {
                let used = cols as u16 * m.card_w + (cols as u16 - 1) * m.gutter;
                assert!(used <= width, "{cols} columns in {width}: {used}");
            }
        }
    }
}

#[test]
fn a_big_graph_places_every_card_without_overlap() {
    // Sixty cards, five columns, a pseudo-random web of edges.
    let mut seed = 17u64;
    let mut next = move || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };
    let depths: Vec<u32> = (0..60).map(|i| (i % 5) as u32).collect();
    let mut edges = Vec::new();
    for _ in 0..90 {
        let a = next() % 60;
        let b = next() % 60;
        edges.push((a, b));
    }
    let i = input(&depths, &edges);
    let (l, g) = place(&i, 200);
    assert!(g.cards.iter().all(Option::is_some));
    assert_no_overlap(&g);
    let in_columns = l
        .columns
        .iter()
        .flatten()
        .filter(|s| matches!(s, Slot::Node(_)))
        .count();
    assert_eq!(in_columns + l.shelf.len(), 60);
    // Reading order visits every card once.
    let mut order = reading_order(&l);
    order.sort_unstable();
    assert_eq!(order, (0..60).collect::<Vec<_>>());
}

#[test]
fn an_empty_graph_is_nothing_and_does_not_panic() {
    let i = Input::default();
    let l = layered(&i);
    assert!(l.columns.is_empty() && l.shelf.is_empty());
    let g = geometry(&i, &l, fit(0, 100).unwrap(), 100, None);
    assert!(g.cards.is_empty());
    assert!(reading_order(&l).is_empty());
}

#[test]
fn lines_use_the_right_glyphs_and_dots_for_contracts_and_claims() {
    let mut i = input(&[0, 1, 0, 1], &[]);
    for (from, to, kind) in [
        (0, 1, EdgeKind::TaskDependency),
        (2, 3, EdgeKind::ContractLink),
    ] {
        i.edges.push(InEdge {
            from,
            to,
            kind,
            state: DepState::Pending,
        });
    }
    let (_, g) = place(&i, 120);
    let m = g.metrics.unwrap();
    let mid = |n: usize| {
        let p = g.cards[n].unwrap();
        (p.x + m.card_w, p.y + m.card_h / 2)
    };
    let (x, y) = mid(0);
    assert_eq!(g.canvas.cell(x, y).glyph(), '─', "a dependency is solid");
    let (x, y) = mid(2);
    assert_eq!(g.canvas.cell(x, y).glyph(), '┈', "a contract is dotted");
}

#[test]
fn a_jog_between_rows_is_drawn_with_corners_in_a_lane() {
    // 0 and 1 both feed 3: one of them has to jog.
    let i = input(&[0, 0, 1, 1], &[(0, 3), (1, 3)]);
    let (_, g) = place(&i, 120);
    let m = g.metrics.unwrap();
    let x0 = m.card_w;
    let glyphs: String = (0..g.canvas.height)
        .flat_map(|y| (x0..x0 + m.gutter).map(move |x| (x, y)))
        .map(|(x, y)| g.canvas.cell(x, y).glyph())
        .collect();
    assert!(glyphs.contains('┐') || glyphs.contains('┘'), "{glyphs}");
    assert!(glyphs.contains('▶'));
}

#[test]
fn the_edges_of_the_selected_card_are_marked() {
    let i = input(&[0, 1, 1], &[(0, 1)]);
    let l = layered(&i);
    let m = fit(2, 120).unwrap();
    let hot = |sel| {
        let g = geometry(&i, &l, m, 120, sel);
        let p = g.cards[0].unwrap();
        g.canvas
            .cell(p.x + m.card_w, p.y + m.card_h / 2)
            .look()
            .unwrap()
            .2
    };
    assert!(hot(Some(1)));
    assert!(hot(Some(0)));
    assert!(!hot(Some(2)));
    assert!(!hot(None));
}

#[test]
fn following_keeps_the_selected_card_in_view() {
    // Already in view: nothing moves.
    assert_eq!(follow(10, 12, 17, 20), 10);
    // Above the view: scroll up to its top.
    assert_eq!(follow(10, 4, 9, 20), 4);
    // Below it: scroll down just enough.
    assert_eq!(follow(0, 30, 35, 20), 15);
    // Taller than the view: its top.
    assert_eq!(follow(0, 3, 40, 10), 3);
    assert_eq!(follow(5, 3, 8, 0), 0);
}

#[test]
fn neighbours_are_the_producers_and_consumers_in_column_order() {
    let i = input(&[0, 0, 1, 2], &[(0, 2), (1, 2), (2, 3)]);
    let l = layered(&i);
    let (before, after) = neighbours(&l, 2);
    assert_eq!(before.len(), 2);
    assert_eq!(after, [3]);
    assert!(neighbours(&l, 0).0.is_empty());
}
