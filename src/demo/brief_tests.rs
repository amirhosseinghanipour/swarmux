//! The Return brief over the demo: the script plays first (the "night"),
//! then the app attaches, and the brief reads the ledger and the console
//! snapshot the way it does for real.

use swarmux_domain::console::{BriefConfig, EscGoesTo};
use swarmux_tui::App;

use super::{LABEL, Options, build};

fn tick(app: &mut App) {
    app.update();
    app.frame += 1;
}

fn away() -> App {
    let mut app = build(&Options {
        away_minutes: Some(60),
        ..Options::default()
    })
    .unwrap();
    tick(&mut app);
    app
}

fn no_private_use(out: &str) {
    assert!(
        !out.chars().any(|c| ('\u{E000}'..='\u{F8FF}').contains(&c)),
        "no private-use glyph"
    );
}

#[test]
fn the_demo_attaches_on_the_brief_and_reads_the_night_at_180x50() {
    let app = away();
    let out = app.render_text(180, 50);
    for needle in [
        "WHILE YOU WERE AWAY",
        "decisions waiting",
        "DECISIONS WAITING",
        "start here",
        "Bash git push origin HEAD:main",
        "auto-deny in",
        "LANDED",
        "GUARD WHILE AWAY",
        "routine allows, not listed",
        // The Guard's few held calls, each with its reason, and the
        // standing rule's routine allows as one number.
        "held, waiting",
        "first-run pass 67% (4 of 6)",
        "SPEND AND CHECKS",
        "TIMELINE",
        "WHAT LANDS WHERE",
        LABEL,
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
    // The demo's gate runs its checks in a clean copy, as for real work: the
    // brief has no worktree warning to show.
    assert!(!out.contains("not a clean copy"), "{out}");
    no_private_use(&out);
}

#[test]
fn the_compact_brief_at_120x36_keeps_decisions_and_folds_the_foot() {
    let mut app = away();
    let out = app.render_text(120, 36);
    for needle in ["DECISIONS WAITING", "GUARD WHILE AWAY", "tab:"] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
    assert!(!out.contains("WHAT LANDS WHERE"), "{out}");
    app.press_named("tab");
    tick(&mut app);
    let out = app.render_text(120, 36);
    assert!(
        out.contains("TIMELINE") && out.contains("WHAT LANDS WHERE"),
        "{out}"
    );
    no_private_use(&out);
}

#[test]
fn enter_starts_with_the_first_decision_in_work_and_esc_comes_back() {
    let mut app = away();
    app.press_named("enter");
    tick(&mut app);
    let out = app.render_text(180, 50);
    assert!(out.contains("DECIDE"), "now in Work\n{out}");
    assert!(!out.contains("WHILE YOU WERE AWAY"), "{out}");
    app.press_named("esc");
    tick(&mut app);
    let out = app.render_text(180, 50);
    assert!(
        out.contains("WHILE YOU WERE AWAY"),
        "back on the brief\n{out}"
    );
}

#[test]
fn esc_skips_to_work_by_default_and_to_the_deck_when_configured() {
    let mut app = away();
    app.press_named("esc");
    tick(&mut app);
    assert!(app.render_text(180, 50).contains("DECIDE"), "Work");
    // Skipping leaves nothing behind it: Esc in Work does not come back.
    app.press_named("esc");
    tick(&mut app);
    assert!(!app.render_text(180, 50).contains("WHILE YOU WERE AWAY"));

    let mut app = away();
    app.brief.settings.esc_goes_to = EscGoesTo::Deck;
    app.press_named("esc");
    tick(&mut app);
    assert!(app.render_text(180, 50).contains("TASK GRAPH"), "the Deck");
}

#[test]
fn g_b_opens_the_brief_from_anywhere_and_w_and_d_skip() {
    let mut app = away();
    app.press_char('w');
    tick(&mut app);
    assert!(app.render_text(180, 50).contains("DECIDE"));
    app.press_char('g');
    app.press_char('b');
    tick(&mut app);
    assert!(app.render_text(180, 50).contains("WHILE YOU WERE AWAY"));
    app.press_char('d');
    tick(&mut app);
    assert!(app.render_text(180, 50).contains("TASK GRAPH"));
}

#[test]
fn without_the_away_flag_the_demo_starts_where_it_always_did() {
    let mut app = build(&Options::default()).unwrap();
    tick(&mut app);
    assert!(!app.render_text(180, 50).contains("WHILE YOU WERE AWAY"));
    // But `g b` still shows the first-run brief: there is no earlier run.
    app.press_char('g');
    app.press_char('b');
    tick(&mut app);
    let out = app.render_text(180, 50);
    assert!(out.contains("This is the first run here"), "{out}");
}

#[test]
fn a_brief_turned_off_does_not_open_by_itself() {
    let app = build(&Options::default()).unwrap();
    let at = app.now() - 60 * 60_000;
    let mut app = app.with_away_since(at).with_brief_config(&BriefConfig {
        present: true,
        show_on_attach: Some(false),
        ..Default::default()
    });
    tick(&mut app);
    assert!(!app.render_text(180, 50).contains("WHILE YOU WERE AWAY"));
}
