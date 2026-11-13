//! The Work screen over the demo: the same services, gate, Guard, ledger
//! and stored streams the app runs on, played headless and drawn.

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

/// `g w`, then a frame.
fn on_work(app: &mut App) {
    app.press_char('g');
    app.press_char('w');
    tick(app);
}

#[test]
fn work_shows_the_held_push_with_the_agents_own_words_and_the_refused_edit() {
    let mut app = played();
    on_work(&mut app);
    let out = app.render_text(180, 50);
    for needle in [
        // The queue, ranked: the held push (P1) first.
        "DECIDE 2",
        "P1",
        "Add retry with backoff to the payment form submit",
        "auto-deny in",
        "LANDED 1",
        "Fix invoice rounding",
        // The selected outcome is the held push.
        "HELD BY GUARD",
        "THE CALL GUARD HELD",
        "Bash git push origin HEAD:main",
        "destructive",
        "typed confirm",
        // The agent's own words, quoted from its stored stream.
        "AGENT'S OWN WORDS",
        "“Refused. I will push the branch with the test as it",
        "LAST STEPS",
        // The protected-test edit the Guard refused, with the words before it.
        "THE CALL GUARD REFUSED",
        "Edit tests/checkout_oracle.rs",
        "protected path tests/**",
        "The retry test is in the way. I will edit its",
        // The answers, from the keymap.
        "ACTIONS",
        "allow once",
        "deny",
        LABEL,
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
}

#[test]
fn work_shows_the_delivered_mission_with_its_proof() {
    let mut app = played();
    on_work(&mut app);
    // Down to the landed mission: the decisions, then the landed group.
    for _ in 0..2 {
        app.press_char('j');
    }
    tick(&mut app);
    let out = app.render_text(180, 50);
    for needle in [
        "Fix invoice rounding",
        "DELIVERED",
        "Invoice totals round half up; the suite passes.",
        "cargo test",
        "gate passed",
        "EVIDENCE",
        "ledger #",
        "signature",
        "evidence",
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
    // Delivered work is never landed twice, and the action says so.
    assert!(out.contains("the work is delivered"), "{out}");
}

#[test]
fn work_fits_a_small_window_and_the_tab_counts_the_decisions() {
    let mut app = played();
    on_work(&mut app);
    let out = app.render_text(120, 36);
    assert_eq!(out.lines().count(), 36);
    assert!(
        out.contains("g w work 2"),
        "the tab counts the decisions\n{out}"
    );
    for needle in ["DECIDE 2", "HELD BY GUARD", "AGENT'S OWN WORDS", "ACTIONS"] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
}

#[test]
fn work_draws_the_same_in_two_runs() {
    let (mut a, mut b) = (played(), played());
    on_work(&mut a);
    on_work(&mut b);
    assert_eq!(a.render_buffer(180, 50), b.render_buffer(180, 50));
}

// ---------------------------------------------------------------------
// C3: per-file counts, the verification figures, send back
// ---------------------------------------------------------------------

fn press(app: &mut App, name: &str) {
    app.press_named(name);
}

fn typed(app: &mut App, text: &str) {
    for c in text.chars() {
        app.press_char(c);
    }
}

#[test]
fn the_delivered_missions_diff_tab_lists_each_file_with_its_lines() {
    let mut app = played();
    on_work(&mut app);
    for _ in 0..2 {
        app.press_char('j');
    }
    app.press_char('3');
    tick(&mut app);
    let out = app.render_text(180, 50);
    for needle in [
        "sealed diff 3 files +41 -9",
        "FILES",
        "from the sealed bundle",
        "M src/invoice/round.rs",
        "+22 -5",
        "M src/invoice/total.rs",
        "+10 -2",
        "M src/invoice/mod.rs",
        "+9 -2",
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
}

#[test]
fn work_shows_how_verification_has_gone_from_the_ledger() {
    let mut app = played();
    on_work(&mut app);
    let out = app.render_text(180, 50);
    assert!(out.contains("last 24h"), "{out}");
    // The mission's gate blocked a premature claim and then passed: one job
    // verified, its first claim not a pass, its checks and its bundle counted.
    for needle in [
        "first-run 0% (0/1)",
        "1 verified",
        "claim→verified",
        "sealed",
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
}

#[test]
fn a_held_call_is_denied_with_a_note_after_the_user_confirms() {
    let mut app = played();
    on_work(&mut app);
    // The push is held and selected first.
    app.press_char('s');
    let out = app.render_text(180, 50);
    for needle in [
        "DENY WITH A NOTE",
        "your note is the reason the agent reads",
        "deny Bash git push origin HEAD:main with a note",
        "nothing is sent before you press enter",
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
    // A suggestion fills the line; it is not sent.
    press(&mut app, "tab");
    let out = app.render_text(180, 50);
    assert!(out.contains("Read the first failing line"), "{out}");
    assert!(
        out.contains("DENY WITH A NOTE"),
        "still composing, nothing sent\n{out}"
    );
    // Clear it and write our own, then confirm.
    for _ in 0..120 {
        press(&mut app, "backspace");
    }
    typed(&mut app, "open a pull request instead");
    press(&mut app, "enter");
    tick(&mut app);
    let out = app.render_text(180, 50);
    assert!(
        out.contains("call denied, your note goes with it · ledger #"),
        "{out}"
    );
    assert!(!out.contains("DENY WITH A NOTE"), "the line closed\n{out}");
}

#[test]
fn escape_drops_the_note_and_sends_nothing() {
    let mut app = played();
    on_work(&mut app);
    app.press_char('s');
    typed(&mut app, "never mind");
    press(&mut app, "esc");
    tick(&mut app);
    let out = app.render_text(180, 50);
    assert!(out.contains("not sent"), "{out}");
    assert!(!out.contains("DENY WITH A NOTE"), "{out}");
    // The call is still held.
    assert!(out.contains("HELD BY GUARD"), "{out}");
}

#[test]
fn an_empty_note_is_refused_and_the_call_stays_held() {
    let mut app = played();
    on_work(&mut app);
    app.press_char('s');
    press(&mut app, "enter");
    tick(&mut app);
    let out = app.render_text(180, 50);
    assert!(out.contains("a note needs words"), "{out}");
    assert!(out.contains("HELD BY GUARD"), "{out}");
}
