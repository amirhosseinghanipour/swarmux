use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use swarmux_application::GateRecord;
use swarmux_application::GateState;
use swarmux_application::evidence::testing::{EvidenceKit, pr};
use swarmux_application::ports::RunnerMsg;
use swarmux_application::testing::asks;
use swarmux_domain::evidence::PrState;
use swarmux_domain::gate::{CheckRun, GateFindings, GateOrigin, GateOutcome};
use swarmux_domain::mission::{Acceptance, Budget, Constraints, Contract, MissionState};
use swarmux_domain::{AgentEvent, HookEvent, JobId, SessionId};

use crate::app::tests_support::{TestApp, test_app};
use crate::app::{App, InputPurpose, Mode, Screen};
use crate::ui::look::IconSet;
use crate::ui::render_to_string;

fn press(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn type_str(app: &mut App, s: &str) {
    for c in s.chars() {
        press(app, KeyCode::Char(c));
    }
}

fn init(id: &str) -> RunnerMsg {
    RunnerMsg::Event(AgentEvent::Init {
        session_id: id.into(),
        model: "claude-sonnet".into(),
        cwd: "/work".into(),
        permission_mode: "default".into(),
    })
}

fn result(text: &str) -> RunnerMsg {
    RunnerMsg::Event(AgentEvent::Result {
        is_error: false,
        subtype: "success".into(),
        cost_usd: 0.25,
        duration_ms: 2500,
        num_turns: 1,
        tokens_in: 1200,
        tokens_out: 300,
        cache_read: 0,
        cache_creation: None,
        context_window: None,
        subagents_spawned: 0,
        text: text.into(),
    })
}

/// Two finished jobs in session "demo", on the ledger screen.
fn with_history() -> TestApp {
    let mut t = test_app();
    t.with_session("demo");
    t.send("Fix the parser");
    let sid = t.runner.request(0).session_id.to_string();
    t.runner.emit(0, init(&sid));
    t.runner.emit(0, result("parser fixed"));
    t.app.update();
    t.clock.advance(60_000);
    t.send("write the docs");
    t.runner.emit(1, RunnerMsg::SpawnFailed("no claude".into()));
    t.app.update();
    press(&mut t.app, KeyCode::Char('5'));
    t
}

#[test]
fn table_and_detail() {
    let t = with_history();
    assert_eq!(t.app.screen, Screen::Ledger);
    let out = render_to_string(&t.app, 140, 16);
    assert!(out.contains("ledger (UTC)"), "{out}");
    assert!(out.contains("01-01 00:01"), "{out}");
    let docs = out.find("write the docs").unwrap();
    let parser = out.find("Fix the parser").unwrap();
    assert!(docs < parser, "newest first: {out}");
    assert!(out.contains("$0.25"), "{out}");
    assert!(out.contains("job #2"), "{out}");
    assert!(out.contains("spawn failed: no claude"), "{out}");
    assert!(out.contains("/ search"), "{out}");
}

#[test]
fn detail_follows_selection_and_shows_result() {
    let mut t = with_history();
    press(&mut t.app, KeyCode::Down);
    let out = render_to_string(&t.app, 140, 16);
    assert!(out.contains("job #1"), "{out}");
    assert!(out.contains("parser fixed"), "{out}");
    assert!(out.contains("2.5s · $0.2500"), "{out}");
    assert!(out.contains("in 1.2k / out 300"), "{out}");
}

#[test]
fn detail_shows_the_review() {
    use swarmux_application::ports::JobRepository;
    use swarmux_domain::{Review, ReviewState, ReviewVerdict};
    let mut t = with_history();
    let review = ReviewState::Done {
        review: Review {
            verdict: ReviewVerdict::Concerns,
            concerns: vec!["no test for empty input".into()],
            diffs_checked: 2,
        },
        job: JobId(9),
        cost_usd: 0.0311,
    };
    t.store.set_review(JobId(1), &review).unwrap();
    press(&mut t.app, KeyCode::Char('5'));
    press(&mut t.app, KeyCode::Down);
    let out = render_to_string(&t.app, 160, 40);
    assert!(
        out.contains("concerns · 2 diffs checked · job #9 · $0.0311"),
        "{out}"
    );
    assert!(out.contains("- no test for empty input"), "{out}");
}

#[test]
fn search_filters_as_you_type_and_esc_clears() {
    let mut t = with_history();
    press(&mut t.app, KeyCode::Char('/'));
    assert!(matches!(t.app.mode, Mode::Input(InputPurpose::Search, _)));
    type_str(&mut t.app, "PARSER");
    assert_eq!(t.app.ledger.rows.len(), 1);
    assert_eq!(t.app.ledger.cards.len(), 1);
    assert!(!t.app.quit, "q-like keys type into the search");
    let out = render_to_string(&t.app, 140, 16);
    assert!(out.contains("search: PARSER"), "{out}");
    press(&mut t.app, KeyCode::Enter);
    assert_eq!(t.app.mode, Mode::Normal);
    assert!(render_to_string(&t.app, 140, 16).contains("1 found"));

    press(&mut t.app, KeyCode::Char('/'));
    press(&mut t.app, KeyCode::Esc);
    assert_eq!(t.app.ledger.rows.len(), 2);
    assert!(t.app.ledger.query.is_empty());
}

#[test]
fn rerun_queues_the_prompt_again() {
    let mut t = with_history();
    press(&mut t.app, KeyCode::Char('r'));
    assert_eq!(t.app.error, None);
    assert_eq!(t.runner.started(), 3);
    assert_eq!(t.runner.request(2).prompt, "write the docs");
    assert_eq!(t.app.ledger.rows.len(), 3);
}

#[test]
fn fork_creates_a_new_session_once_claude_reports_it() {
    let mut t = with_history();
    press(&mut t.app, KeyCode::Down);
    press(&mut t.app, KeyCode::Char('f'));
    let req = t.runner.request(2);
    assert!(req.fork && req.resume);
    let new_id = SessionId::random();
    t.runner.emit(2, init(&new_id.to_string()));
    t.app.update();
    let names: Vec<&str> = t.app.sessions.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"demo fork"), "{names:?}");
    assert_eq!(t.app.selected_session().unwrap().name, "demo");
}

#[test]
fn enter_replays_a_job_from_an_earlier_run() {
    let t = with_history();
    // A new app over the same store: its jobs are history, not live.
    let mut t2 = crate::app::tests_support::test_app_with_store(t.store.clone());
    press(&mut t2.app, KeyCode::Char('5'));
    press(&mut t2.app, KeyCode::Down);
    press(&mut t2.app, KeyCode::Enter);
    assert_eq!(t2.app.screen, Screen::Dashboard);
    let out = render_to_string(&t2.app, 160, 48);
    assert!(out.contains("done"), "{out}");
    assert!(out.contains("$0.2500"), "{out}");
}

#[test]
fn the_ledger_is_screen_five_and_history_is_gone() {
    let mut t = test_app();
    press(&mut t.app, KeyCode::Char('5'));
    let out = render_to_string(&t.app, 120, 36);
    assert!(out.contains("5 ledger"), "{out}");
    assert!(!out.contains("history"), "{out}");
    assert!(out.contains("no runs yet"), "{out}");
    assert!(out.contains("no mission has ended yet"), "{out}");
}

// --- evidence --------------------------------------------------------------

fn contract(title: &str, goal: &str, checks: &[&str], usd: f64) -> Contract {
    Contract {
        title: title.into(),
        goal: goal.into(),
        acceptance: Acceptance {
            checks: checks.iter().map(|c| c.to_string()).collect(),
            human_signoff: false,
        },
        constraints: Constraints {
            paths_rw: vec!["src/**".into()],
            protected: vec!["tests/**".into()],
        },
        budget: Budget {
            usd: Some(usd),
            wall_minutes: Some(90),
            ..Budget::default()
        },
        clarifications: Vec::new(),
        repos: Vec::new(),
    }
}

fn finish(t: &TestApp, i: usize, cost: f64, ok: bool) {
    t.runner.emit(
        i,
        RunnerMsg::Event(AgentEvent::Result {
            is_error: !ok,
            subtype: "success".into(),
            cost_usd: cost,
            duration_ms: 95_000,
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

/// What the gate did at a stop of `job`, on the app and on the ledger.
fn stop(
    t: &mut TestApp,
    kit: &EvidenceKit,
    job: JobId,
    session: SessionId,
    outcome: GateOutcome,
    checks: &[&str],
) {
    let ok = outcome == GateOutcome::Pass;
    let runs: Vec<CheckRun> = checks
        .iter()
        .enumerate()
        .map(|(i, c)| CheckRun {
            name: c.chars().take(32).collect(),
            command: c.to_string(),
            exit: Some(i32::from(!ok && i == 0)),
            timed_out: false,
            spawn_error: None,
            duration_ms: 4_200 + 900 * i as u64,
            output: if ok || i > 0 {
                String::new()
            } else {
                "thread 'retry' panicked at tests/retry.rs:31".into()
            },
            flaky: false,
        })
        .collect();
    let at = t.app.now();
    t.app.mission.gates.insert(
        job,
        GateState {
            outcome,
            blocks: u32::from(outcome == GateOutcome::Block),
            max_blocks: 3,
            checks: runs
                .iter()
                .map(|r| (r.name.clone(), r.passed(), false))
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
    let record = GateRecord {
        job: Some(job),
        session: "claude-session".into(),
        event: Some(HookEvent::Stop),
        root: "/work".into(),
        origin: GateOrigin::File,
        outcome,
        blocks: 0,
        max_blocks: 3,
        findings: GateFindings {
            runs,
            ..GateFindings::default()
        },
        digest: String::new(),
        problem: None,
    };
    kit.ledger_service
        .record_gate(&record, Some(session), at)
        .unwrap();
}

/// A ledger with two delivered missions (one after a blocked stop), one
/// escalated, one running and one plain job; labels asked and answered.
fn scenario() -> (TestApp, EvidenceKit) {
    let mut t = test_app();
    let kit = t.with_evidence();
    for name in ["search", "api", "parser", "web", "docs"] {
        t.with_session(name);
        t.clock.advance(1_000);
    }
    let sid = |t: &TestApp, n: &str| t.app.sessions.iter().find(|s| s.name == n).unwrap().id;
    let specs = [
        (
            "Fast search",
            "make search answer in under 50 ms on the bench corpus",
            &["cargo test -p search", "cargo clippy -p search"][..],
            2.0,
            "search",
        ),
        (
            "Fix the flaky retry test",
            "make the http retry test pass 50 runs in a row without sleeping",
            &["cargo test -p http retry", "cargo clippy -p http"][..],
            1.5,
            "api",
        ),
        (
            "Rewrite the config parser",
            "replace the hand-written config parser with the toml crate",
            &["cargo test -p config"][..],
            0.5,
            "parser",
        ),
        (
            "Add the export command",
            "add an export command that writes the ledger as json files",
            &["cargo test -p export"][..],
            3.0,
            "web",
        ),
    ];
    for (title, goal, checks, usd, session) in specs {
        t.clock.advance(1_000);
        let s = sid(&t, session);
        t.app
            .mission_service()
            .create(contract(title, goal, checks, usd), "/work", Some(s))
            .unwrap();
    }
    for i in 0..4 {
        t.linter.reply(i, asks(&[]));
    }
    t.app.update();
    for i in 0..4 {
        t.app.screen = Screen::Missions;
        t.app.missions_screen.selected = i;
        t.keys([KeyCode::Char('r')]);
        assert_eq!(t.app.error, None);
    }
    t.app.update();
    t.clock.advance(60_000);
    let job = |t: &TestApp, i: usize| t.app.missions()[i].job.unwrap();
    let session_of = |t: &TestApp, i: usize| t.app.missions()[i].session.unwrap();
    // Fast search: the gate blocked once, then passed.
    let (j0, s0) = (job(&t, 0), session_of(&t, 0));
    stop(
        &mut t,
        &kit,
        j0,
        s0,
        GateOutcome::Block,
        &["cargo test -p search", "cargo clippy -p search"],
    );
    t.clock.advance(30_000);
    finish(&t, 0, 0.84, true);
    stop(
        &mut t,
        &kit,
        j0,
        s0,
        GateOutcome::Pass,
        &["cargo test -p search", "cargo clippy -p search"],
    );
    // The retry fix passed at once.
    let (j1, s1) = (job(&t, 1), session_of(&t, 1));
    t.clock.advance(5_000);
    finish(&t, 1, 1.12, true);
    stop(
        &mut t,
        &kit,
        j1,
        s1,
        GateOutcome::Pass,
        &["cargo test -p http retry", "cargo clippy -p http"],
    );
    // The parser job failed.
    finish(&t, 2, 0.31, false);
    // The export command is still running (job 3).
    t.app.update();
    t.app.update();
    // A plain job, no mission.
    t.app.screen = Screen::Dashboard;
    t.clock.advance(10_000);
    t.send("tidy the docs");
    finish(&t, 4, 0.05, true);
    t.app.update();
    t.app.update();
    assert_eq!(t.app.missions()[0].state, MissionState::Delivered);
    assert_eq!(t.app.missions()[1].state, MissionState::Delivered);
    assert_eq!(t.app.missions()[2].state, MissionState::Escalated);
    // Labels: the first merged, the second still open.
    t.app.screen = Screen::Ledger;
    t.keys([KeyCode::Char('l')]);
    assert_eq!(kit.probe.requests().len(), 2);
    for (i, req) in kit.probe.requests().iter().enumerate() {
        let state = if req.slug == "fast-search" {
            PrState::Merged { at: 61_000 }
        } else {
            PrState::Open
        };
        kit.probe.reply(i, Ok(Some(pr(40 + i as u64, state))));
    }
    t.app.update();
    (t, kit)
}

/// Waits for a background verification to be pumped into the app.
fn verified(t: &mut TestApp) {
    let end = Instant::now() + Duration::from_secs(5);
    while t.app.evidence().is_some_and(|e| e.verifying("fast-search")) {
        assert!(Instant::now() < end, "verification never finished");
        std::thread::sleep(Duration::from_millis(5));
        t.app.update();
    }
    t.app.update();
}

/// Newest first: the plain job, the export, the parser, the retry fix,
/// then Fast search.
fn select_fast_search(t: &mut TestApp) {
    for _ in 0..4 {
        press(&mut t.app, KeyCode::Down);
    }
}

fn assert_fits(out: &str, w: usize) {
    for line in out.lines() {
        assert!(
            line.chars().count() <= w,
            "{} wide: {line}",
            line.chars().count()
        );
    }
}

#[test]
fn every_delivered_mission_has_a_bundle_and_a_card() {
    let (t, kit) = scenario();
    let stored = kit.service.list().unwrap();
    let slugs: Vec<&str> = stored
        .iter()
        .map(|s| s.bundle.contract.slug.as_str())
        .collect();
    assert_eq!(slugs, ["fast-search", "fix-the-flaky-retry-test"]);
    let delivered = t
        .app
        .missions()
        .iter()
        .filter(|m| m.state == MissionState::Delivered)
        .count();
    assert_eq!(delivered, stored.len());
    let cards = &t.app.ledger.cards;
    assert_eq!(cards.len(), 5);
    assert_eq!(
        cards.iter().filter(|c| c.evidence.is_some()).count(),
        2,
        "{cards:?}"
    );
    let fast = stored
        .iter()
        .find(|s| s.bundle.contract.slug == "fast-search")
        .unwrap();
    assert_eq!(fast.bundle.blocks(), 1);
    assert_eq!(fast.bundle.checks.len(), 2);
}

#[test]
fn ledger_screen_at_120x36() {
    let (mut t, _kit) = scenario();
    select_fast_search(&mut t);
    let out = render_to_string(&t.app, 120, 36);
    assert_fits(&out, 120);
    for needle in [
        "5 ledger",
        "the record",
        "pass^3 0.30",
        "2/3 delivered",
        "/verified outcome",
        "interventions/task",
        "last 24 h: ",
        "needs you: Rewrite the config parser",
        "ledger (UTC) · all",
        "Fast search",
        "delivered",
        "escalated",
        "running",
        "no bundle",
        "evidence bundle",
        "signed (hash only)",
        "cargo test -p search",
        "1 block(s), then pass",
        "sonnet+haiku",
        "merged #40",
        "pr open #41",
        "v re-runs the checks at head",
    ] {
        assert!(out.contains(needle), "missing {needle:?} in\n{out}");
    }
}

#[test]
fn ledger_screen_at_160x48() {
    let (mut t, _kit) = scenario();
    select_fast_search(&mut t);
    let out = render_to_string(&t.app, 160, 48);
    assert_fits(&out, 160);
    for needle in [
        "the record",
        "pass^3 0.30",
        "2/3 delivered",
        "last 24 h: ",
        "Add the export command",
        "tidy the docs",
        "evidence bundle",
        "head     cafe0000cafe on swarmux/demo/1",
        "diff     2 file(s) +30 -4",
        "cost     $0.84 (reported)",
        "ledger   #",
        "label    merged #40",
    ] {
        assert!(out.contains(needle), "missing {needle:?} in\n{out}");
    }
}

#[test]
fn ascii_icons_keep_the_screen_readable() {
    let (mut t, _kit) = scenario();
    t.app.look.icons = IconSet::Ascii;
    let out = render_to_string(&t.app, 120, 36);
    assert_fits(&out, 120);
    assert!(out.is_ascii() || !out.contains('✓'), "{out}");
    assert!(out.contains("+ delivered"), "{out}");
}

#[test]
fn v_verifies_the_bundle_at_head_in_a_clean_checkout() {
    let (mut t, kit) = scenario();
    // Newest first: the plain job, the export, the parser, the retry fix,
    // then Fast search.
    for _ in 0..4 {
        press(&mut t.app, KeyCode::Down);
    }
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains("job #1"), "{out}");
    press(&mut t.app, KeyCode::Char('v'));
    verified(&mut t);
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains("match"), "{out}");
    assert!(out.contains("items hold"), "{out}");
    let asked = kit.checkouts.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].1, "cafe0000cafe0000");
    assert!(
        kit.checker
            .roots
            .lock()
            .unwrap()
            .iter()
            .all(|r| r == std::path::Path::new("/clean"))
    );
}

#[test]
fn a_check_that_fails_on_the_rerun_is_a_mismatch_on_screen() {
    let (mut t, kit) = scenario();
    for _ in 0..4 {
        press(&mut t.app, KeyCode::Down);
    }
    kit.checker.exit("cargo clippy -p search", 1);
    press(&mut t.app, KeyCode::Char('v'));
    verified(&mut t);
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains("mismatch"), "{out}");
    assert!(
        out.contains("check cargo clippy -p search: expected pass, found fail"),
        "{out}"
    );
}

#[test]
fn v_does_nothing_on_a_run_without_a_bundle() {
    let (mut t, kit) = scenario();
    press(&mut t.app, KeyCode::Char('v'));
    assert!(kit.checkouts.asked.lock().unwrap().is_empty());
    assert_eq!(t.app.error, None);
}

#[test]
fn the_filter_narrows_the_cards() {
    let (mut t, _kit) = scenario();
    press(&mut t.app, KeyCode::Char('o'));
    assert_eq!(t.app.ledger.cards.len(), 2, "delivered");
    press(&mut t.app, KeyCode::Char('o'));
    assert_eq!(t.app.ledger.cards.len(), 1, "needs you");
    press(&mut t.app, KeyCode::Char('o'));
    assert_eq!(t.app.ledger.cards.len(), 1, "labelled: the merged one");
    let out = render_to_string(&t.app, 120, 36);
    assert!(out.contains("ledger (UTC) · labelled"), "{out}");
    press(&mut t.app, KeyCode::Char('o'));
    assert_eq!(t.app.ledger.cards.len(), 5);
    // The search also finds a run by its outcome or label.
    press(&mut t.app, KeyCode::Char('/'));
    type_str(&mut t.app, "merged");
    assert_eq!(t.app.ledger.cards.len(), 1);
}

#[test]
fn the_digest_covers_the_time_away() {
    let (mut t, kit) = scenario();
    // Last here before any of it happened, and a lot of time has passed.
    t.clock.advance(3 * 3_600_000);
    t.app.attach_evidence(kit.service.clone(), Some(0));
    press(&mut t.app, KeyCode::Char('1'));
    press(&mut t.app, KeyCode::Char('5'));
    let out = render_to_string(&t.app, 120, 36);
    assert!(out.contains("while you were away: "), "{out}");
    assert!(out.contains("2 delivered"), "{out}");
    assert!(out.contains("1 escalated"), "{out}");
    assert!(out.contains("1 merged"), "{out}");
    // Seen a minute ago: the usual last-day brief.
    t.app
        .attach_evidence(kit.service.clone(), Some(t.app.now() - 60_000));
    press(&mut t.app, KeyCode::Char('5'));
    assert!(render_to_string(&t.app, 120, 36).contains("last 24 h: "));
}

#[test]
fn small_windows_skip_the_numbers_strip() {
    let (t, _kit) = scenario();
    let out = render_to_string(&t.app, 100, 15);
    assert_fits(&out, 100);
    assert!(!out.contains("the record"), "{out}");
    assert!(
        out.contains("Fast search") || out.contains("tidy the docs"),
        "{out}"
    );
}

#[test]
fn the_hints_name_the_ledger_keys() {
    let mut t = test_app();
    press(&mut t.app, KeyCode::Char('5'));
    let hints = crate::ui::hints(&t.app);
    for key in [
        "/ search", "o filter", "v verify", "l labels", "r re-run", "f fork",
    ] {
        assert!(hints.contains(key), "{hints}");
    }
}
