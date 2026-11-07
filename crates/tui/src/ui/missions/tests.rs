use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use swarmux_application::GateState;
use swarmux_application::ports::RunnerMsg;
use swarmux_application::testing::asks;
use swarmux_domain::gate::GateOutcome;
use swarmux_domain::mission::{Acceptance, Budget, Constraints, Contract, MissionState};
use swarmux_domain::{AgentEvent, JobId, SessionId};

use crate::app::Screen;
use crate::app::tests_support::{TestApp, test_app};
use crate::ui::look::IconSet;
use crate::ui::render_to_string;

fn ctrl(t: &mut TestApp, c: char) {
    t.app
        .on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
}

fn typed(t: &mut TestApp, s: &str) {
    t.keys(s.chars().map(KeyCode::Char));
}

fn session(t: &TestApp, name: &str) -> SessionId {
    t.app.sessions.iter().find(|s| s.name == name).unwrap().id
}

fn contract(title: &str, goal: &str, checks: &[&str], protected: &[&str], usd: f64) -> Contract {
    Contract {
        title: title.into(),
        goal: goal.into(),
        acceptance: Acceptance {
            checks: checks.iter().map(|c| c.to_string()).collect(),
            human_signoff: false,
        },
        constraints: Constraints {
            paths_rw: vec!["src/**".into()],
            protected: protected.iter().map(|c| c.to_string()).collect(),
        },
        budget: Budget {
            usd: Some(usd),
            wall_minutes: Some(90),
            ..Budget::default()
        },
        clarifications: Vec::new(),
    }
}

/// Ends the `i`-th started job with `cost` spent.
fn finish(t: &TestApp, i: usize, cost: f64, ok: bool) {
    t.runner.emit(
        i,
        RunnerMsg::Event(AgentEvent::Result {
            is_error: !ok,
            subtype: "success".into(),
            cost_usd: cost,
            duration_ms: 60_000,
            num_turns: 12,
            tokens_in: 1_000,
            tokens_out: 4_000,
            cache_read: 0,
            cache_creation: None,
            context_window: None,
            subagents_spawned: 0,
            text: "done".into(),
        }),
    );
    t.runner.emit(
        i,
        RunnerMsg::Exited {
            code: Some(if ok { 0 } else { 1 }),
            stderr_tail: String::new(),
        },
    );
}

fn gate(t: &mut TestApp, job: JobId, outcome: GateOutcome, checks: &[(&str, bool)]) {
    let at = t.app.now();
    t.app.mission.gates.insert(
        job,
        GateState {
            outcome,
            blocks: u32::from(outcome == GateOutcome::Block),
            max_blocks: 3,
            checks: checks
                .iter()
                .map(|(n, ok)| (n.to_string(), *ok, false))
                .collect(),
            protected_changed: 0,
            evidence_gaps: 0,
            hidden: None,
            tampering: 0,
            checks_weakened: false,
            base_suite: None,
            digest: String::new(),
            problem: None,
            at,
        },
    );
}

/// Four missions: delivered, running with a blocked stop, escalated, and
/// a draft the lint has questions about.
pub(crate) fn scenario() -> TestApp {
    let mut t = test_app();
    for name in ["search", "api", "parser", "web"] {
        t.with_session(name);
        t.clock.advance(1_000);
    }
    let fast = contract(
        "Fast search",
        "make search answer in under 50 ms on the bench corpus",
        &["cargo test -p search", "sh scripts/bench.sh --max-ms 50"],
        &["tests/**", "scripts/bench.sh"],
        2.0,
    );
    let retry = contract(
        "Fix the flaky retry test",
        "make the http retry test pass 50 runs in a row without sleeping",
        &["cargo test -p http retry", "cargo clippy -p http"],
        &["crates/http/tests/**"],
        1.5,
    );
    let parser = contract(
        "Rewrite the config parser",
        "replace the hand-written config parser with the toml crate",
        &["cargo test -p config"],
        &["tests/**"],
        0.5,
    );
    let vague = Contract {
        title: "Make the app faster".into(),
        goal: "make it faster".into(),
        ..Contract::default()
    };
    let ids = ["search", "api", "parser", "web"].map(|n| session(&t, n));
    let slugs: Vec<String> = [fast, retry, parser, vague]
        .into_iter()
        .zip(ids)
        .map(|(c, s)| {
            t.clock.advance(1_000);
            t.app.mission_service().create(c, "/work", Some(s)).unwrap()
        })
        .collect();
    // The model asks about the vague one; the others are clear.
    for i in 0..3 {
        t.linter.reply(i, asks(&[]));
    }
    t.linter.reply(
        3,
        asks(&[
            "What does faster mean: page load, API latency or build time?",
            "Which command shows it is fast enough?",
        ]),
    );
    t.app.update();
    for slug in &slugs[..3] {
        t.app.screen = Screen::Missions;
        let at = t
            .app
            .missions()
            .iter()
            .position(|m| &m.slug == slug)
            .unwrap();
        t.app.missions_screen.selected = at;
        t.keys([KeyCode::Char('r')]);
        assert_eq!(t.app.error, None);
    }
    t.app.update();
    t.clock.advance(60_000);
    let job = |t: &TestApp, i: usize| t.app.missions()[i].job.unwrap();
    // Fast search: done, and the gate passed at its stop.
    finish(&t, 0, 0.84, true);
    let j0 = job(&t, 0);
    gate(
        &mut t,
        j0,
        GateOutcome::Pass,
        &[
            ("cargo test -p search", true),
            ("sh scripts/bench.sh --max-ms 50", true),
        ],
    );
    // The retry fix: still running, its last stop blocked.
    let j1 = job(&t, 1);
    gate(
        &mut t,
        j1,
        GateOutcome::Block,
        &[
            ("cargo test -p http retry", false),
            ("cargo clippy -p http", true),
        ],
    );
    // The parser: the job failed.
    finish(&t, 2, 0.31, false);
    t.app.update();
    t.app.update();
    t.app.screen = Screen::Missions;
    t.app.missions_screen.selected = 3;
    t.app.notice = None;
    t
}

#[test]
fn the_scenario_reaches_each_state() {
    let t = scenario();
    let states: Vec<MissionState> = t.app.missions().iter().map(|m| m.state).collect();
    assert_eq!(
        states,
        [
            MissionState::Delivered,
            MissionState::Running,
            MissionState::Escalated,
            MissionState::Linted,
        ]
    );
    assert_eq!(t.job_gates.assigned.borrow().len(), 3);
}

#[test]
fn snapshot_160x48() {
    let mut t = scenario();
    t.app.look.icons = IconSet::Unicode;
    let out = render_to_string(&t.app, 160, 48);
    println!("{out}");
    for needle in [
        "2 missions",
        "missions 4",
        "✓ 1 delivered",
        "▶ 1 running",
        "⚑ 1 escalated",
        "Fast search",
        "draft › linted › queued › running › verifying › delivered",
        "✓ cargo test -p search",
        "✗ cargo test -p http retry",
        "⛨ 2 protected",
        "budget ",
        "$0.84 of $2.00",
        "◇ single session",
        "job #1 search",
        "⚑ escalated",
        "the job did not finish",
        "3 questions",
        "1 blocking",
        "contract",
        "make it faster",
        "no acceptance checks: nothing can tell that it is done",
        "What does faster mean",
        "unanswered · e answers it",
    ] {
        assert!(out.contains(needle), "missing {needle:?}\n{out}");
    }
}

#[test]
fn every_size_and_icon_set_renders() {
    for icons in [IconSet::Ascii, IconSet::Unicode, IconSet::Nerd] {
        let mut t = scenario();
        t.app.look.icons = icons;
        for (w, h) in [(80, 24), (120, 40), (160, 48), (60, 12), (40, 10)] {
            let out = render_to_string(&t.app, w, h);
            assert!(out.lines().count() == usize::from(h), "{w}x{h}");
        }
        let narrow = render_to_string(&t.app, 80, 24);
        assert!(narrow.contains("missions 4"), "{narrow}");
    }
    let mut t = scenario();
    t.app.look.icons = IconSet::Ascii;
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains("draft > linted"), "{out}");
    assert!(!out.contains('✓'), "ascii only:\n{out}");
}

#[test]
fn no_missions_explains_what_one_is() {
    let mut t = test_app();
    t.app.screen = Screen::Missions;
    let out = render_to_string(&t.app, 120, 30);
    assert!(out.contains("No missions yet."), "{out}");
    assert!(out.contains("Press n to write one"), "{out}");
    t.keys([KeyCode::Char('n')]);
    assert!(t.app.missions_screen.composer.is_none());
    assert!(t.app.error.as_deref().unwrap().contains("press 6 then n"));
}

#[test]
fn the_composer_writes_lints_asks_and_runs() {
    let mut t = test_app();
    t.with_session("search");
    t.keys([KeyCode::Char('2'), KeyCode::Char('n')]);
    typed(&mut t, "Fast search");
    t.keys([KeyCode::Tab]);
    typed(&mut t, "make search answer in under 50 ms");
    t.keys([KeyCode::Tab]);
    typed(&mut t, "cargo test -p search");
    t.keys([KeyCode::Tab, KeyCode::Tab, KeyCode::Tab]);
    typed(&mut t, "$2 90m");
    ctrl(&mut t, 's');
    assert_eq!(t.app.error, None);
    let m = &t.app.missions()[0];
    assert_eq!(
        (m.slug.as_str(), m.state),
        ("fast-search", MissionState::Linted)
    );
    assert_eq!(m.root, "/work");
    assert!(
        t.missions.saves.get() >= 2,
        "drafted and linted are both kept"
    );
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains("mission · fast-search"), "{out}");
    assert!(out.contains("the checks judge by tests/**"), "{out}");
    assert!(out.contains("asking haiku"), "{out}");

    // The model's question appears inline with a field to answer it.
    t.linter
        .reply(0, asks(&["Which corpus is the bench run on?"]));
    t.app.update();
    let composer = t.app.missions_screen.composer.as_ref().unwrap();
    assert_eq!(composer.answers.len(), 1);
    t.keys([KeyCode::Tab]);
    typed(&mut t, "the bench corpus");
    ctrl(&mut t, 's');
    assert_eq!(
        t.linter.requests().len(),
        1,
        "an answer alone is not a new lint"
    );
    assert_eq!(
        t.app.missions()[0].contract.clarifications[0].answer,
        "the bench corpus"
    );
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains("Which corpus is the bench run on?"), "{out}");
    assert!(out.contains("→ the bench corpus"), "{out}");

    // Protect the oracle, then run.
    ctrl(&mut t, 'f');
    assert_eq!(t.app.notice.as_deref(), Some("protected 1 oracle"));
    assert_eq!(
        t.app.missions()[0].contract.constraints.protected,
        ["tests/**"]
    );
    ctrl(&mut t, 'r');
    assert_eq!(t.app.error, None);
    assert!(t.app.missions_screen.composer.is_none());
    let m = &t.app.missions()[0];
    assert_eq!(m.state, MissionState::Queued);
    let prompt = t.runner.request(0).prompt;
    assert!(
        prompt.contains("- Which corpus is the bench run on? the bench corpus"),
        "{prompt}"
    );
    assert_eq!(t.job_gates.assigned.borrow()[0].1.protected, ["tests/**"]);
    // ⏎ follows its job in Focus.
    t.keys([KeyCode::Enter]);
    assert_eq!(t.app.screen, Screen::Dashboard);
}

#[test]
fn a_mission_that_cannot_be_checked_is_refused() {
    let mut t = test_app();
    t.with_session("web");
    t.keys([KeyCode::Char('2'), KeyCode::Char('n')]);
    t.keys([KeyCode::Tab]);
    typed(&mut t, "make it faster");
    ctrl(&mut t, 'r');
    let e = t.app.error.clone().unwrap();
    assert!(e.contains("refused: no acceptance checks"), "{e}");
    assert_eq!(t.app.missions()[0].state, MissionState::Refused);
    assert_eq!(t.runner.started(), 0);
    t.keys([KeyCode::Esc]);
    let out = render_to_string(&t.app, 160, 48);
    assert!(
        out.contains("⊘ refused") || out.contains("x refused"),
        "{out}"
    );
    // e reopens it; fixing the contract brings it back.
    t.keys([KeyCode::Char('e'), KeyCode::Tab]);
    typed(&mut t, "npm run bench");
    ctrl(&mut t, 's');
    assert_eq!(t.app.missions()[0].state, MissionState::Linted);
}

#[test]
fn a_mission_out_cannot_be_edited() {
    let mut t = scenario();
    t.app.missions_screen.selected = 1;
    t.keys([KeyCode::Char('e')]);
    assert!(t.app.missions_screen.composer.is_none());
    assert!(
        t.app
            .notice
            .as_deref()
            .unwrap()
            .contains("can't change now")
    );
    t.keys([KeyCode::Up, KeyCode::Up, KeyCode::Up, KeyCode::Down]);
    assert_eq!(t.app.missions_screen.selected, 1);
}
