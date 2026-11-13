//! The Deck drawn: at 180×50 and 120×36, with a card selected, with more
//! cards than fit, with nothing to show, and with a mistake in `[deck]`.
//! Over the read model's own evening snapshot, with the figures an
//! evening's ledger and usage windows would add.

use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, layout::Rect};
use swarmux_application::console::{
    ConsoleSnapshot, DeckNode, FirstRunView, LimitWindow, MetricsView, NodeKind, NodeState,
    SpreadView,
};
use swarmux_domain::console::{DeckConfig, DeckLayout, Panel, resolve_deck};

use super::{Want, arrange, folds, heights, render_with};
use crate::app::App;
use crate::app::tests_support::test_app;
use crate::deck::Focus;
use crate::work::tests::evening;

const NOW: i64 = 10_000_000;

fn app() -> App {
    let t = test_app();
    t.clock.advance(NOW - 1_000);
    t.app
}

/// The evening, with the figures a real run has and the golden leaves out.
fn snapshot() -> ConsoleSnapshot {
    let mut snap = evening();
    snap.metrics = Some(MetricsView {
        at: NOW,
        window_ms: 24 * 3_600_000,
        covered_ms: 24 * 3_600_000,
        complete: true,
        first_run: Some(FirstRunView {
            passed: 11,
            judged: 18,
            rate: 11.0 / 18.0,
        }),
        claim_to_verified: Some(SpreadView {
            median_ms: 41 * 60_000,
            p90_ms: 112 * 60_000,
            samples: 18,
        }),
        verified: 23,
        verified_per_hour: Some(1.4),
        checks_run: 1_412,
        bundles_sealed: 23,
    });
    snap.header.limits = vec![
        LimitWindow {
            window: "five_hour".into(),
            status: "allowed".into(),
            utilization: Some(0.624),
            resets_at: Some(NOW + 133 * 60_000),
            hits_at: Some(NOW + 123 * 60_000),
            stale: false,
        },
        LimitWindow {
            window: "seven_day".into(),
            status: "allowed".into(),
            utilization: Some(0.41),
            resets_at: Some(NOW + 3 * 86_400_000),
            hits_at: None,
            stale: false,
        },
    ];
    snap.header.spend.today_usd = Some(48.28);
    snap.header.spend.week_usd = Some(212.5);
    snap
}

fn draw(app: &App, snap: &ConsoleSnapshot, w: u16, h: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| render_with(f, f.area(), app, snap))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn text(buf: &Buffer) -> String {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Nothing is drawn in a private-use glyph.
fn assert_clean(buf: &Buffer, at: &str) {
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let s = buf[(x, y)].symbol();
            assert!(
                !s.chars().any(|c| ('\u{E000}'..='\u{F8FF}').contains(&c)),
                "{at}: a private-use glyph at {x},{y}"
            );
        }
    }
}

/// The rows of `buf` inside `area`, to see what a panel holds.
fn region(buf: &Buffer, area: Rect) -> String {
    (area.y..area.y + area.height)
        .map(|y| {
            (area.x..area.x + area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_deck_at_180x50_has_every_panel_and_none_of_them_empty() {
    let app = app();
    let snap = snapshot();
    let buf = draw(&app, &snap, 180, 47);
    let out = text(&buf);
    assert_clean(&buf, "180x47");
    for needle in [
        "TASK GRAPH",
        "producer → consumer",
        "SESSIONS",
        "NEEDS YOU",
        "VERIFICATION",
        "LIMITS · BUDGET",
        "GUARD EXCEPTIONS",
        "TIMELINE",
        // The legend.
        "task dependency, met",
        "cross-repo contract",
        "file claim",
        "blocked or escalated",
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
    // Every panel the design has holds something: look inside each frame.
    let panels = arrange(
        Rect::new(0, 0, 180, 47),
        &DeckLayout::default(),
        None,
        false,
    );
    assert_eq!(panels.len(), 7, "all seven panels are placed");
    for (panel, rect) in panels {
        let inner = Rect::new(rect.x + 1, rect.y + 1, rect.width - 2, rect.height - 2);
        let body = region(&buf, inner);
        let filled = body.lines().filter(|l| !l.trim().is_empty()).count();
        assert!(
            filled >= 1,
            "{panel} is nearly empty ({filled} rows):\n{body}"
        );
    }
}

#[test]
fn the_cards_say_state_checks_and_spend_and_the_lines_end_in_arrows() {
    let app = app();
    let buf = draw(&app, &snapshot(), 180, 47);
    let out = text(&buf);
    // The task graph of the evening: t1 verified-ish, t2 failed, t3 waiting.
    for needle in [
        "t1 DONE",
        "t2 FAILED",
        "t3 WAITING",
        "#3 FAILED",
        "1/2",
        "no checks configured",
        // The graph is a producer-to-consumer picture: arrows into t3.
        "▶",
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
    // The shapes: hollow, solid, diagonal cut, pause bars.
    for shape in ["□", "■", "▚", "▌▐"] {
        assert!(out.contains(shape), "{shape}\n{out}");
    }
    // The job on the held push shows as held, not as running.
    assert!(out.contains("#2 HELD"), "{out}");
}

#[test]
fn a_selected_card_has_a_double_border_and_its_relations_in_words() {
    let mut app = app();
    let snap = snapshot();
    let plan = crate::deck::Plan::of(&snap);
    let t3 = plan.index_of("task:split-app:t3").unwrap();
    app.deck_screen.select(&plan, t3);
    let out = text(&draw(&app, &snap, 180, 47));
    assert!(out.contains('╔') && out.contains('╝'), "{out}");
    // Its edges, all of them, in words.
    assert!(out.contains("← t1 (dep, met)"), "{out}");
    assert!(out.contains("← t2 (dep, blocked)"), "{out}");
    // The same card at 120x36 as a list marks it too.
    let out = text(&draw(&app, &three_columns(), 120, 33));
    assert!(out.contains('▌'), "{out}");
}

#[test]
fn a_file_claim_is_a_dotted_line_between_the_holder_and_the_waiter() {
    let app = app();
    let out = text(&draw(&app, &snapshot(), 180, 47));
    // Job 2 holds src/checkout/** and job 4 waits: a claim edge.
    assert!(out.contains('╌'), "a dotted claim line\n{out}");
    assert!(out.contains("src/checkout/**"), "{out}");
}

/// The snapshot with a third column: a task that waits for t3.
fn three_columns() -> ConsoleSnapshot {
    let mut snap = snapshot();
    let mut t4 = snap
        .deck
        .nodes
        .iter()
        .find(|n| n.id == "task:split-app:t3")
        .unwrap()
        .clone();
    t4.id = "task:split-app:t4".into();
    t4.title = "task t4".into();
    t4.depth = 2;
    snap.deck.nodes.push(t4);
    snap.deck
        .edges
        .push(swarmux_application::console::DeckEdge {
            from: "task:split-app:t3".into(),
            to: "task:split-app:t4".into(),
            kind: swarmux_application::console::EdgeKind::TaskDependency,
            state: swarmux_application::console::DepState::Pending,
            detail: Vec::new(),
        });
    snap.deck.max_depth = 2;
    snap
}

#[test]
fn the_deck_at_120x36_folds_the_graph_to_a_list_and_some_panels_away() {
    let app = app();
    let buf = draw(&app, &three_columns(), 120, 33);
    let out = text(&buf);
    assert_clean(&buf, "120x33");
    assert!(folds(120, 33));
    for needle in [
        "TASK GRAPH",
        "as a list at this size",
        "NEEDS YOU",
        "LIMITS · BUDGET",
        "GUARD EXCEPTIONS",
        "#2",
        "t3",
        "→ t4 (dep, pending)",
    ] {
        assert!(
            out.contains(needle),
            "{needle}
{out}"
        );
    }
    // These fold: they come back one at a time with the more key.
    for gone in ["SESSIONS", "VERIFICATION", "TIMELINE"] {
        assert!(
            !out.contains(gone),
            "{gone} should fold
{out}"
        );
    }
}

#[test]
fn two_columns_still_fit_as_a_graph_at_120x36() {
    let app = app();
    let out = text(&draw(&app, &snapshot(), 120, 33));
    assert!(out.contains("producer → consumer"), "{out}");
    assert!(out.contains('▶'), "{out}");
}

#[test]
fn the_folded_panels_take_the_graphs_place_one_at_a_time() {
    let mut app = app();
    let snap = snapshot();
    let mut seen = Vec::new();
    for _ in 0..4 {
        let out = text(&draw(&app, &snap, 120, 33));
        seen.push(if out.contains("SESSIONS") {
            "sessions"
        } else if out.contains("VERIFICATION") {
            "verification"
        } else if out.contains("TIMELINE") {
            "timeline"
        } else {
            "graph"
        });
        app.deck_more();
    }
    assert_eq!(seen, ["graph", "sessions", "verification", "timeline"]);
    assert_eq!(app.deck_screen.folded, None, "after the last, the graph");
}

#[test]
fn verification_shows_the_sample_size_and_keeps_unknown_unknown() {
    let app = app();
    let mut snap = snapshot();
    let out = text(&draw(&app, &snap, 180, 47));
    assert!(out.contains("61% (11 of 18)"), "{out}");
    assert!(out.contains("median 41m · p90 1h52m · n=18"), "{out}");
    assert!(out.contains("1,412") || out.contains("1412"), "{out}");

    // Three judged jobs: the percentage never stands alone.
    if let Some(m) = &mut snap.metrics {
        m.first_run = Some(FirstRunView {
            passed: 2,
            judged: 3,
            rate: 2.0 / 3.0,
        });
        m.claim_to_verified = None;
    }
    let out = text(&draw(&app, &snap, 180, 47));
    assert!(out.contains("67% (2 of 3) · small n"), "{out}");
    assert!(out.contains("no verified claim yet"), "{out}");

    // Nothing judged: unknown, not 0%.
    if let Some(m) = &mut snap.metrics {
        m.first_run = None;
    }
    let out = text(&draw(&app, &snap, 180, 47));
    assert!(out.contains("no first claim judged"), "{out}");
    assert!(!out.contains("0% ·"), "{out}");

    // No ledger at all: every figure is unknown.
    snap.metrics = None;
    let out = text(&draw(&app, &snap, 180, 47));
    assert!(out.contains("the figures are unknown"), "{out}");
}

#[test]
fn limits_show_the_forecast_and_spend_that_is_unknown_says_so() {
    let app = app();
    let mut snap = snapshot();
    let out = text(&draw(&app, &snap, 180, 47));
    assert!(out.contains("5-HOUR WINDOW"), "{out}");
    assert!(out.contains("62.4%"), "{out}");
    assert!(out.contains("forecast: cap in 2h03m"), "{out}");
    assert!(out.contains("$48.28"), "{out}");
    snap.header.limits.clear();
    snap.header.spend.today_usd = None;
    let out = text(&draw(&app, &snap, 180, 47));
    assert!(out.contains("usage limits are not read yet"), "{out}");
    assert!(
        out.contains("SPEND TODAY") && out.contains("unknown"),
        "{out}"
    );
}

#[test]
fn the_guard_panel_lists_exceptions_with_their_rule_and_counts_the_rest_as_one_number() {
    let app = app();
    let out = text(&draw(&app, &snapshot(), 180, 47));
    assert!(out.contains("HELD"), "{out}");
    assert!(out.contains("DENIED"), "{out}");
    assert!(out.contains("Bash git push origin HEAD:main"), "{out}");
    assert!(out.contains("decision in Work"), "{out}");
    assert!(out.contains("protected path tests/**"), "{out}");
    assert!(out.contains("by rule:"), "{out}");
    // No decisions were made in this app: the routine number says so.
    assert!(out.contains("nothing decided yet"), "{out}");
}

#[test]
fn needs_you_lists_the_decisions_with_why_and_the_way_to_work() {
    let app = app();
    let out = text(&draw(&app, &snapshot(), 180, 47));
    assert!(out.contains("NEEDS YOU"), "{out}");
    assert!(out.contains("4 · in Work"), "{out}");
    assert!(out.contains("P1"), "{out}");
    assert!(out.contains("auto-deny in"), "{out}");
    assert!(out.contains("Work, or"), "{out}");
}

#[test]
fn with_nothing_to_show_it_says_so_in_a_line_not_a_blank_screen() {
    let app = app();
    let mut snap = snapshot();
    snap.deck.nodes.clear();
    snap.deck.edges.clear();
    snap.groups.iter_mut().for_each(|g| {
        g.outcomes.clear();
        g.total = 0;
    });
    snap.holds.clear();
    snap.header.counts.decide = 0;
    for (w, h) in [(180, 47), (120, 33)] {
        let buf = draw(&app, &snap, w, h);
        let out = text(&buf);
        assert!(out.contains("no jobs yet"), "{w}x{h}\n{out}");
        assert!(out.contains("nothing waits for you"), "{w}x{h}\n{out}");
        assert!(
            out.contains("no call is held or refused") || w == 120,
            "{w}x{h}\n{out}"
        );
        assert_clean(&buf, "empty");
    }
}

#[test]
fn many_cards_scroll_and_the_selected_one_is_always_in_view() {
    let mut app = app();
    let mut snap = snapshot();
    // Forty more cards in a chain of tasks: long columns and a long list.
    for i in 0..40 {
        let mut n: DeckNode = snap.deck.nodes[0].clone();
        n.id = format!("job:{}", 100 + i);
        n.kind = NodeKind::Job;
        n.job = Some(100 + i);
        n.title = format!("extra job {i}");
        n.state = NodeState::Running;
        n.group = None;
        n.outcome = n.id.clone();
        snap.deck.nodes.push(n);
    }
    let plan = crate::deck::Plan::of(&snap);
    for (w, h) in [(180u16, 47u16), (120, 33)] {
        for last in [false, true] {
            let target = if last {
                plan.order.last().copied().unwrap()
            } else {
                plan.order[0]
            };
            app.deck_screen.select(&plan, target);
            let buf = draw(&app, &snap, w, h);
            let out = text(&buf);
            assert_clean(&buf, "many");
            let want = &snap.deck.nodes[target];
            let name = if want.title.starts_with("extra job") {
                want.title.clone()
            } else {
                want.id.rsplit(':').next().unwrap().to_string()
            };
            assert!(
                out.contains(&name),
                "{w}x{h} last={last}: the selected card ({name}) is in view\n{out}"
            );
        }
    }
    // The graph says how many cards are out of view.
    app.deck_screen.select(&plan, plan.order[0]);
    let out = text(&draw(&app, &snap, 180, 47));
    assert!(out.contains("more out of view"), "{out}");
}

#[test]
fn a_bad_deck_config_is_said_above_the_panels_and_what_is_wrong_is_left_out() {
    let config = DeckConfig {
        present: true,
        panels: Some(vec!["graph".into(), "weather".into(), "needs".into()]),
        ..DeckConfig::default()
    };
    let app = app().with_deck_config(&config);
    let buf = draw(&app, &snapshot(), 180, 47);
    let out = text(&buf);
    assert_clean(&buf, "bad config");
    assert!(
        out.lines()
            .next()
            .is_some_and(|l| l.contains("[deck] panels: unknown panel \"weather\"")
                && l.contains("swarmux deck lint")),
        "{out}"
    );
    // The panels that were named show; the others do not.
    assert!(
        out.contains("TASK GRAPH") && out.contains("NEEDS YOU"),
        "{out}"
    );
    for gone in ["SESSIONS", "VERIFICATION", "LIMITS", "GUARD", "TIMELINE"] {
        assert!(!out.contains(gone), "{gone} is not in panels\n{out}");
    }
}

#[test]
fn the_order_of_panels_in_a_zone_is_the_order_in_the_config() {
    let mut layout = resolve_deck(&DeckConfig {
        present: true,
        panels: Some(vec![
            "limits".into(),
            "needs".into(),
            "sessions".into(),
            "graph".into(),
            "timeline".into(),
            "guard".into(),
        ]),
        ..DeckConfig::default()
    });
    assert!(layout.problems.is_empty());
    let placed = arrange(Rect::new(0, 0, 180, 47), &layout, None, false);
    let at = |p: Panel| placed.iter().find(|(q, _)| *q == p).unwrap().1;
    // Main column: sessions above the graph. Side: limits above needs.
    // Foot: timeline left of guard.
    assert!(at(Panel::Sessions).y < at(Panel::Graph).y);
    assert!(at(Panel::Limits).y < at(Panel::Needs).y);
    assert!(at(Panel::Timeline).x < at(Panel::Guard).x);
    assert!(!placed.iter().any(|(p, _)| *p == Panel::Verification));
    // A zone with no panels gives its room away: no side column, the main
    // column takes the width.
    layout.panels = vec![Panel::Graph];
    let placed = arrange(Rect::new(0, 0, 180, 47), &layout, None, false);
    assert_eq!(placed, [(Panel::Graph, Rect::new(0, 0, 180, 47))]);
}

#[test]
fn panels_never_overlap_and_stay_inside_the_area_at_any_size() {
    let layout = DeckLayout::default();
    for (w, h) in [
        (80, 16),
        (100, 24),
        (120, 33),
        (139, 47),
        (140, 39),
        (140, 40),
        (180, 47),
        (240, 70),
    ] {
        let area = Rect::new(0, 0, w, h);
        for folded in [None, Some(Panel::Sessions), Some(Panel::Verification)] {
            let placed = arrange(area, &layout, folded, folds(w, h));
            for (i, (p, a)) in placed.iter().enumerate() {
                assert!(
                    a.x + a.width <= w && a.y + a.height <= h,
                    "{p} at {w}x{h} leaves the area: {a:?}"
                );
                for (q, b) in &placed[i + 1..] {
                    let apart = a.x + a.width <= b.x
                        || b.x + b.width <= a.x
                        || a.y + a.height <= b.y
                        || b.y + b.height <= a.y;
                    assert!(apart, "{p} and {q} overlap at {w}x{h}: {a:?} {b:?}");
                }
            }
            assert!(
                placed.iter().any(|(p, _)| matches!(
                    p,
                    Panel::Graph | Panel::Sessions | Panel::Verification | Panel::Timeline
                )),
                "a main panel at {w}x{h}"
            );
        }
    }
}

#[test]
fn heights_give_each_its_minimum_then_its_ideal_then_the_rest_to_the_growers() {
    let wants = [
        Want {
            min: 6,
            ideal: 9,
            grow: 1,
        },
        Want {
            min: 8,
            ideal: 9,
            grow: 0,
        },
        Want {
            min: 9,
            ideal: 15,
            grow: 2,
        },
    ];
    // Just the minimums.
    assert_eq!(heights(23, &wants), [6, 8, 9]);
    // Ideals in order.
    assert_eq!(heights(33, &wants), [9, 9, 15]);
    // The rest by weight: 1 : 2 of 30 left.
    let h = heights(63, &wants);
    assert_eq!(h.iter().sum::<u16>(), 63);
    assert!(h[2] > h[0], "{h:?}");
    assert_eq!(h[1], 9, "a panel that does not grow stays at its ideal");
    // Not even the minimums fit: the tallest give rows, all still fit.
    let h = heights(15, &wants);
    assert!(h.iter().sum::<u16>() <= 15, "{h:?}");
    assert!(h.iter().all(|x| *x >= 3));
    assert_eq!(heights(10, &[]), Vec::<u16>::new());
}

#[test]
fn too_small_asks_for_a_bigger_window() {
    let app = app();
    let out = text(&draw(&app, &snapshot(), 70, 14));
    assert!(out.contains("make the window bigger"), "{out}");
    assert!(out.contains("80×16"), "{out}");
}

#[test]
fn the_sessions_strip_marks_the_selected_row_when_it_has_the_keys() {
    let mut app = app();
    app.deck_screen.focus = Focus::Sessions;
    let out = text(&draw(&app, &snapshot(), 180, 47));
    // No session is attached in this app: it says how to make one.
    assert!(out.contains("no sessions yet"), "{out}");
}
