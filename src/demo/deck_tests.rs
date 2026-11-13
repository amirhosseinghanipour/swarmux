//! The Deck over the demo: the same services, gate, Guard, ledger and
//! stored streams the app runs on, played headless and drawn.

use swarmux_tui::App;

use super::{LABEL, Options, build};

/// Ticks of the whole scenario, plus a little for the last job to settle.
const TICKS: u32 = 340;

fn tick(app: &mut App) {
    app.update();
    app.frame += 1;
}

fn played() -> App {
    let mut app = build(&Options::default()).unwrap();
    for _ in 0..TICKS {
        tick(&mut app);
    }
    app
}

/// `g d`, then a frame.
fn on_deck(app: &mut App) {
    app.press_char('g');
    app.press_char('d');
    tick(app);
}

#[test]
fn g_d_goes_to_the_deck_and_the_demo_fills_every_panel_at_180x50() {
    let mut app = played();
    on_deck(&mut app);
    let out = app.render_text(180, 50);
    for needle in [
        // The header navigation lists the deck first among the new screens.
        "g d deck",
        // The graph: the release's tasks, with the dependency into t3.
        "TASK GRAPH",
        "t1 DONE",
        "t2 DONE",
        "t3 HELD",
        "Backfill the events table",
        "Regenerate the API reference",
        "▶",
        // A job on its own, the verified one, and the held push.
        "#5 VERIFIED",
        "#4 HELD",
        // The sessions strip: five sessions, what they ran.
        "SESSIONS",
        "5 attached",
        "api-gateway",
        "data-pipeline",
        "mobile-app",
        // What needs the user.
        "NEEDS YOU",
        "2 · in Work",
        "auto-deny in",
        // Verification, from the ledger: the sample size is beside the rate.
        "VERIFICATION",
        "jobs verified",
        "first-run pass",
        "(0 of 1) · small n",
        "claim→verified",
        "1 bundle sealed",
        // Limits and spend, as far as they are known.
        "LIMITS · BUDGET",
        "5-HOUR WINDOW",
        "forecast: cap in",
        "SPEND TODAY",
        "release-2-4",
        // The Guard: the held calls with their reason, and the routine
        // allows as one number.
        "GUARD EXCEPTIONS",
        "Bash git push origin HEAD:main",
        "decision in Work",
        "routine allows, not listed",
        // The timeline.
        "TIMELINE",
        "done $",
        LABEL,
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
    assert!(
        !out.chars().any(|c| ('\u{E000}'..='\u{F8FF}').contains(&c)),
        "no private-use glyph"
    );
}

#[test]
fn the_compact_deck_at_120x36_keeps_the_graph_and_folds_the_rest() {
    let mut app = played();
    on_deck(&mut app);
    let out = app.render_text(120, 36);
    for needle in [
        "TASK GRAPH",
        "t3 HELD",
        "NEEDS YOU",
        "LIMITS · BUDGET",
        "GUARD EXCEPTIONS",
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
    for folded in ["VERIFICATION", "TIMELINE"] {
        assert!(!out.contains(folded), "{folded} folds\n{out}");
    }
    // Tab brings the folded panels, one at a time, in the graph's place.
    app.press_named("tab");
    tick(&mut app);
    let out = app.render_text(120, 36);
    assert!(
        out.contains("SESSIONS") && !out.contains("TASK GRAPH"),
        "{out}"
    );
}

#[test]
fn enter_on_a_card_opens_its_outcome_in_work_and_esc_comes_back() {
    let mut app = played();
    on_deck(&mut app);
    // The first card in reading order is the release's first task: its
    // outcome is the release mission.
    app.press_named("enter");
    tick(&mut app);
    let out = app.render_text(180, 50);
    assert!(out.contains("DECIDE 2"), "now in Work\n{out}");
    assert!(
        out.contains("Release 2.4"),
        "the release is selected\n{out}"
    );
    app.press_named("esc");
    tick(&mut app);
    let out = app.render_text(180, 50);
    assert!(out.contains("TASK GRAPH"), "back on the deck\n{out}");
}

#[test]
fn the_keys_walk_the_graph_by_edges_and_by_reading_order() {
    let mut app = played();
    on_deck(&mut app);
    // t1 → its consumer t3 with the right key, back with the left.
    app.press_char('l');
    tick(&mut app);
    let out = app.render_text(180, 50);
    assert!(out.contains("t3 Regenerate the API reference"), "{out}");
    assert!(out.contains("← t1 (dep, met)"), "{out}");
    app.press_char('h');
    tick(&mut app);
    let out = app.render_text(180, 50);
    assert!(out.contains("t1 Backfill the events table"), "{out}");
    // j and k step through every card.
    for _ in 0..8 {
        app.press_char('j');
    }
    tick(&mut app);
    assert!(app.render_text(180, 50).contains("TASK GRAPH"));
}

#[test]
fn a_bad_deck_config_is_said_on_the_deck_and_the_demo_still_draws() {
    use swarmux_domain::console::DeckConfig;
    let app = build(&Options::default())
        .unwrap()
        .with_deck_config(&DeckConfig {
            present: true,
            panels: Some(vec!["graph".into(), "weather".into()]),
            ..DeckConfig::default()
        });
    let mut app = app;
    for _ in 0..TICKS {
        tick(&mut app);
    }
    on_deck(&mut app);
    let out = app.render_text(180, 50);
    assert!(out.contains("unknown panel \"weather\""), "{out}");
    assert!(out.contains("TASK GRAPH"), "{out}");
    assert!(!out.contains("GUARD EXCEPTIONS"), "{out}");
}
