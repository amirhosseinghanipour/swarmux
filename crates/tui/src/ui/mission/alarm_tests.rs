//! The alarm-grade inbox on screen and under the keys: tiers, batches,
//! shelving, focus mode, floods, the typed confirmation and the load hold.

use crossterm::event::KeyCode;
use swarmux_application::ports::Clock;
use swarmux_domain::{AttentionKind, Priority};

use super::tests::scenario;
use crate::app::tests_support::TestApp;
use crate::app::{Mode, Screen};
use crate::ui::look::IconSet;
use crate::ui::render_to_string;

const SEC: i64 = 1_000;
const MIN: i64 = 60 * SEC;

fn scene() -> TestApp {
    let mut t = scenario();
    t.app.look.icons = IconSet::Unicode;
    t
}

/// Moves the selection down until `want` is selected.
fn select(t: &mut TestApp, want: impl Fn(&swarmux_domain::AttentionItem) -> bool) {
    for _ in 0..40 {
        if t.app.mission.selected_item().is_some_and(&want) {
            return;
        }
        t.keys([KeyCode::Char('j')]);
    }
    panic!("the item is not in the inbox: {:?}", t.app.mission.items);
}

fn allowed(t: &TestApp) -> Vec<u64> {
    t.inbox
        .answers()
        .into_iter()
        .filter(|(_, d)| d.verdict == swarmux_domain::Verdict::Allow)
        .map(|(id, _)| id)
        .collect()
}

/// A dozen asks of one kind from the same job, as a build loop makes them.
fn plant_test_runs(t: &mut TestApp, n: usize) -> Vec<u64> {
    let job = t.runner.request(1).job_id;
    let ids: Vec<u64> = (0..n)
        .map(|k| {
            t.inbox
                .push(Some(job), "Bash", &format!("cargo test case_{k}"))
        })
        .collect();
    t.clock.advance(SEC);
    t.app.update();
    ids
}

#[test]
fn the_selected_irreversible_ask_shows_its_tier_and_no_shortcut() {
    let t = scene();
    let out = render_to_string(&t.app, 160, 48);
    println!("{out}");
    assert!(
        out.contains("y confirm (name the target) · n deny"),
        "{out}"
    );
    assert!(out.contains("R4 destructive"), "{out}");
    assert!(
        !out.contains("a always · ⏎ open · s shelve"),
        "no always-allow for an irreversible call"
    );
    // The overlay says the same, and the other ask reads as routine.
    assert!(out.contains("approval needed"), "{out}");
    assert!(out.contains("R4 destructive · score"), "{out}");
    assert!(out.contains("certain · routine"), "{out}");
    // Every ask was counted by priority as it arrived.
    let k = t.app.mission.alarms().kpis(t.clock.now());
    assert_eq!(k.distribution[0], 1);
    assert!(k.distribution[1] >= 2);
}

#[test]
fn a_flood_shows_the_first_cause_and_one_key_answers_the_batch() {
    let mut t = scene();
    let ids = plant_test_runs(&mut t, 12);
    let out = render_to_string(&t.app, 160, 48);
    println!("{out}");
    assert!(out.contains("flood: 14 items from 3 causes"), "{out}");
    assert!(out.contains("first out shown"), "{out}");
    assert!(
        out.contains("×12"),
        "the first ask carries its batch\n{out}"
    );
    assert!(
        !out.contains("case_7"),
        "consequentials are suppressed\n{out}"
    );
    assert!(t.app.mission.alarms().kpis(t.clock.now()).flood);

    // The destructive ask is still on top.
    assert_eq!(t.app.mission.items[0].priority, Priority::P1);

    // One key answers the batch, but not the ask that is P1.
    t.keys([KeyCode::Char('A')]);
    assert!(
        allowed(&t).is_empty(),
        "A on a P1 ask does nothing: {:?}",
        t.inbox.answers()
    );
    select(&mut t, |i| i.batch == 12);
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains("A all 12"), "{out}");
    t.keys([KeyCode::Char('A')]);
    let got = allowed(&t);
    for id in &ids {
        assert!(got.contains(id), "ask {id} was approved");
    }
    assert_eq!(got.len(), 12, "nothing else was answered");
    let k = t.app.mission.alarms().kpis(t.clock.now());
    assert!(k.attended >= 1 && k.ttd_median.is_some(), "looks are timed");
}

#[test]
fn the_irreversible_ask_arms_for_a_moment_then_wants_its_target_named() {
    let mut t = scene();
    t.keys([KeyCode::Char('y')]);
    assert!(matches!(t.app.mode, Mode::Confirm(_)), "y opens the dialog");
    assert!(allowed(&t).is_empty(), "nothing is approved yet");
    let out = render_to_string(&t.app, 160, 48);
    println!("{out}");
    assert!(out.contains("irreversible · confirm"), "{out}");
    assert!(
        out.contains("R4 Bash git push origin feat/checkout"),
        "{out}"
    );
    assert!(out.contains("arming…"), "{out}");

    // Keys typed while it arms are ignored, Enter included.
    t.keys("feat/checkout".chars().map(KeyCode::Char));
    t.keys([KeyCode::Enter]);
    assert!(matches!(t.app.mode, Mode::Confirm(_)));
    assert!(allowed(&t).is_empty());

    t.clock.advance(1_500);
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains("type the target"), "{out}");
    assert!(!out.contains("arming…"));

    // The wrong name is refused and the field is cleared.
    t.keys("main".chars().map(KeyCode::Char));
    t.keys([KeyCode::Enter]);
    assert!(matches!(t.app.mode, Mode::Confirm(_)));
    assert!(render_to_string(&t.app, 160, 48).contains("not the target"));
    assert!(allowed(&t).is_empty());

    // The right one goes through, and is logged.
    t.keys("feat/checkout".chars().map(KeyCode::Char));
    t.keys([KeyCode::Enter]);
    assert_eq!(t.app.mode, Mode::Normal);
    assert_eq!(allowed(&t), [1]);
}

#[test]
fn the_target_can_be_picked_from_a_rotated_list_and_esc_keeps_the_call_held() {
    let mut t = scene();
    t.keys([KeyCode::Char('y')]);
    t.clock.advance(1_500);
    t.keys([KeyCode::Tab]);
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains("pick the target"), "{out}");
    let Mode::Confirm(c) = &t.app.mode else {
        panic!("no dialog");
    };
    let right = c
        .challenge
        .options()
        .iter()
        .position(|o| o == "feat/checkout")
        .expect("the target is on the list")
        + 1;
    let wrong = if right == 1 { 2 } else { 1 };
    t.keys([KeyCode::Char(
        char::from_digit(wrong as u32, 10).unwrap_or('1'),
    )]);
    assert!(allowed(&t).is_empty(), "a decoy approves nothing");
    t.keys([KeyCode::Esc]);
    assert_eq!(t.app.mode, Mode::Normal);
    assert!(allowed(&t).is_empty(), "esc keeps it held");
    assert!(t.app.engine.guard.pending_count() >= 2);

    t.keys([KeyCode::Char('y')]);
    t.clock.advance(1_500);
    t.keys([KeyCode::Tab]);
    let Mode::Confirm(c) = &t.app.mode else {
        panic!("no dialog");
    };
    let right = c
        .challenge
        .options()
        .iter()
        .position(|o| o == "feat/checkout")
        .expect("the target is on the list")
        + 1;
    t.keys([KeyCode::Char(
        char::from_digit(right as u32, 10).unwrap_or('1'),
    )]);
    assert_eq!(allowed(&t), [1]);
}

#[test]
fn deny_is_immediate_and_always_is_refused_for_irreversible_calls() {
    let mut t = scene();
    t.keys([KeyCode::Char('a')]);
    assert_eq!(t.app.mode, Mode::Normal);
    assert!(t.rules.rules().is_empty());
    assert!(t.inbox.answers().is_empty());
    t.keys([KeyCode::Char('n')]);
    assert_eq!(t.inbox.answers().len(), 1);
    assert!(allowed(&t).is_empty());
}

#[test]
fn shelving_hides_an_item_and_it_comes_back_by_itself() {
    let mut t = scene();
    select(&mut t, |i| i.kind == AttentionKind::JobFailed);
    let id = t.app.mission.selected_item().map(|i| i.id.clone()).unwrap();
    t.keys([KeyCode::Char('s')]);
    assert_eq!(t.app.mode, Mode::Shelve);
    assert!(render_to_string(&t.app, 160, 48).contains("15 min"));
    t.keys([KeyCode::Char('1')]);
    assert_eq!(t.app.mode, Mode::Normal);
    assert!(t.app.mission.items.iter().all(|i| i.id != id));
    let out = render_to_string(&t.app, 160, 48);
    println!("{out}");
    assert!(out.contains("shelved 1"), "{out}");

    t.clock.advance(14 * MIN);
    t.app.update();
    assert!(t.app.mission.items.iter().all(|i| i.id != id), "not yet");
    t.clock.advance(MIN);
    t.app.update();
    assert!(t.app.mission.items.iter().any(|i| i.id == id), "back");
    assert!(!render_to_string(&t.app, 160, 48).contains("shelved 1"));
}

#[test]
fn p1_cannot_be_shelved() {
    let mut t = scene();
    select(&mut t, |i| i.priority == Priority::P1);
    t.keys([KeyCode::Char('s')]);
    assert_eq!(t.app.mode, Mode::Normal);
    assert!(
        t.app
            .mission
            .items
            .iter()
            .any(|i| i.priority == Priority::P1)
    );
    assert!(t.app.mission.alarms().shelved_items().is_empty());
}

#[test]
fn focus_mode_counts_what_it_holds_and_never_hides_p1() {
    let mut t = scene();
    t.keys([KeyCode::Char('F')]);
    let out = render_to_string(&t.app, 160, 48);
    println!("{out}");
    assert!(out.contains("waiting · next ≤"), "{out}");
    let ids = |t: &TestApp| -> Vec<String> {
        t.app
            .mission
            .items
            .iter()
            .map(|i| i.id.to_string())
            .collect()
    };
    // The destructive ask shows at once; the fresh routine ask waits; the
    // failure is long past its two minutes and shows.
    assert_eq!(ids(&t), ["approval:1", "job:4:failed"]);

    // Switching tiles is a breakpoint: P2 comes through, P3 does not.
    t.keys([KeyCode::Tab, KeyCode::Char('j'), KeyCode::Tab]);
    assert_eq!(ids(&t), ["approval:1", "approval:2", "job:4:failed"]);

    // P3 waits for the end of focus, but never longer than its bound.
    t.clock.advance(36 * MIN);
    t.app.update();
    assert!(
        t.app
            .mission
            .items
            .iter()
            .any(|i| i.priority == Priority::P3)
    );

    t.keys([KeyCode::Char('F')]);
    assert!(!t.app.mission.alarms().focus());
    assert!(t.app.mission.items.len() >= 3);
}

#[test]
fn working_the_inbox_holds_new_work_and_resting_lets_it_go() {
    let mut t = scene();
    t.with_session("late");
    t.keys([KeyCode::Char('1')]);
    // A long stretch of keys in the inbox fills the whole window.
    for _ in 0..40 {
        t.clock.advance(30 * SEC);
        t.keys([KeyCode::Char('j')]);
        t.app.update();
    }
    let load = t
        .app
        .load
        .share(t.clock.now())
        .expect("watched long enough");
    assert!(load > 0.7, "{load}");
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains("load "), "{out}");

    let before = t.runner.started();
    t.send("one more thing");
    t.keys([KeyCode::Char('1')]);
    assert_eq!(t.runner.started(), before, "held");
    let held = t
        .app
        .queue_rows()
        .into_iter()
        .find(|r| r.prompt == "one more thing")
        .and_then(|r| r.note)
        .expect("the queue says why");
    assert!(held.starts_with("held: you're at "), "{held}");
    assert!(held.ends_with("% load"), "{held}");
    assert_eq!(t.app.stats().held, 1);

    // Away from the inbox, the load drains and the job starts.
    t.keys([KeyCode::Char('4')]);
    assert_eq!(t.app.screen, Screen::Queue);
    for _ in 0..30 {
        t.clock.advance(MIN);
        t.app.update();
    }
    assert_eq!(t.runner.started(), before + 1);
    assert_eq!(t.app.stats().held, 0);
}

#[test]
fn ten_clean_approvals_offer_a_rule_and_r_writes_it() {
    let mut t = scene();
    let job = t.runner.request(1).job_id;
    for k in 0..10 {
        let id = t
            .inbox
            .push(Some(job), "Bash", &format!("cargo fmt pass_{k}"));
        t.clock.advance(SEC);
        t.app.update();
        select(&mut t, |i| i.request == Some(id));
        t.keys([KeyCode::Char('y')]);
    }
    let id = t.inbox.push(Some(job), "Bash", "cargo fmt again");
    t.clock.advance(SEC);
    t.app.update();
    select(&mut t, |i| i.request == Some(id));
    let out = render_to_string(&t.app, 160, 48);
    println!("{out}");
    assert!(out.contains("r make it a rule? (10/10)"), "{out}");
    t.keys([KeyCode::Char('r')]);
    let kept = t.rules.rules();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].text, "Bash(cargo fmt:*)");
    assert_eq!(kept[0].source.as_str(), "suggestion");
    assert_eq!(
        kept[0].scope.as_str(),
        "repo",
        "saved where its tally came from"
    );
    assert!(allowed(&t).contains(&id), "the held ask is covered by it");
    assert!(
        !render_to_string(&t.app, 160, 48).contains("make it a rule"),
        "offered once"
    );
}
