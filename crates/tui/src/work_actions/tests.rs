//! The Work screen's keys through a real app: the answers go to the call
//! of the selected outcome and nobody else's, and the actions run the
//! services the old screens ran.

use crossterm::event::KeyCode;
use swarmux_domain::JobStatus;

use crate::app::Screen;
use crate::app::tests_support::{TestApp, test_app};

fn chars(t: &mut TestApp, text: &str) {
    t.keys(text.chars().map(KeyCode::Char));
}

/// Two sessions with a job each; the first holds a call.
fn held_and_running() -> TestApp {
    let mut t = test_app();
    t.with_session("alpha");
    t.send("first");
    let job = t.app.current_job().unwrap().job_id;
    t.with_session("beta");
    t.send("second");
    t.inbox.push(Some(job), "Bash", "make deploy");
    t.app.update();
    t
}

#[test]
fn g_w_goes_to_work_and_the_tab_counts_the_decisions() {
    let mut t = held_and_running();
    chars(&mut t, "gw");
    assert_eq!(t.app.screen, Screen::Work);
    let out = t.app.render_text(160, 40);
    assert!(out.contains("work 1"), "{out}");
    assert!(out.contains("DECIDE 1"), "{out}");
    assert!(out.contains("HELD BY GUARD"), "{out}");
}

#[test]
fn y_answers_the_call_of_the_selected_outcome() {
    let mut t = held_and_running();
    chars(&mut t, "gw");
    assert_eq!(t.app.engine.guard.pending_count(), 1);
    chars(&mut t, "y");
    t.app.update();
    assert_eq!(
        t.app.engine.guard.pending_count(),
        0,
        "the call was approved"
    );
}

#[test]
fn y_on_an_outcome_with_nothing_held_does_not_answer_another_ones_call() {
    let mut t = held_and_running();
    chars(&mut t, "gw");
    // Down to the running job: it holds nothing.
    chars(&mut t, "j");
    chars(&mut t, "y");
    assert_eq!(
        t.app.engine.guard.pending_count(),
        1,
        "the other call stays held"
    );
    assert_eq!(
        t.app.notice.as_deref(),
        Some("no call is held on this outcome")
    );
    // And n and a do not reach it either.
    chars(&mut t, "n");
    chars(&mut t, "a");
    assert_eq!(t.app.engine.guard.pending_count(), 1);
}

#[test]
fn n_denies_the_selected_call() {
    let mut t = held_and_running();
    chars(&mut t, "gw");
    chars(&mut t, "n");
    t.app.update();
    assert_eq!(t.app.engine.guard.pending_count(), 0);
}

#[test]
fn kill_stops_the_selected_running_job() {
    let mut t = held_and_running();
    chars(&mut t, "gw");
    chars(&mut t, "j");
    let snap = t.app.console_current().unwrap().clone();
    let selected = t.app.work_selected().unwrap();
    assert_eq!(
        snap.groups.iter().map(|g| g.outcomes.len()).sum::<usize>(),
        2
    );
    let job = selected.jobs[0].id;
    chars(&mut t, "x");
    t.app.update();
    let status = t
        .app
        .engine
        .jobs
        .progress(swarmux_domain::JobId(job))
        .map(|p| p.status);
    assert_eq!(status, Some(JobStatus::Cancelled), "{:?}", t.app.notice);
}

#[test]
fn an_action_that_cannot_run_says_why_and_changes_nothing() {
    let mut t = held_and_running();
    chars(&mut t, "gw");
    // Landing is for a mission: a plain job is not offered it.
    chars(&mut t, "l");
    assert_eq!(t.app.screen, Screen::Work);
    assert_eq!(
        t.app.notice.as_deref(),
        Some("that does not apply to this outcome")
    );
    assert_eq!(t.app.engine.guard.pending_count(), 1);
    // Retry on a job that has not ended.
    t.app.notice = None;
    chars(&mut t, "r");
    assert_eq!(
        t.app.notice.as_deref(),
        Some("that does not apply to this outcome")
    );
}

#[test]
fn enter_opens_the_job_in_focus_and_the_tabs_switch() {
    let mut t = held_and_running();
    chars(&mut t, "gw");
    chars(&mut t, "2");
    assert_eq!(t.app.work_screen.tab, crate::work::Tab::Stream);
    chars(&mut t, "4");
    assert_eq!(t.app.work_screen.tab, crate::work::Tab::Checks);
    chars(&mut t, "1");
    assert_eq!(t.app.work_screen.tab, crate::work::Tab::Product);
    t.keys([KeyCode::Enter]);
    assert_eq!(t.app.screen, Screen::Dashboard);
    // Esc goes back to Work.
    t.keys([KeyCode::Esc]);
    assert_eq!(t.app.screen, Screen::Work);
}

#[test]
fn rebinding_a_work_key_moves_it_and_the_actions_panel_shows_the_new_key() {
    let mut t = held_and_running();
    t.app = t
        .app
        .with_key_config(&crate::keys::config_of(&[("work", "work.kill", &["K"])]));
    chars(&mut t, "gw");
    chars(&mut t, "j");
    let out = t.app.render_text(160, 40);
    assert!(
        out.lines().any(|l| l.contains(" K ") && l.contains("kill")),
        "{out}"
    );
    assert!(t.app.key_of(crate::screens::work::KILL).as_deref() == Some("K"));
}
