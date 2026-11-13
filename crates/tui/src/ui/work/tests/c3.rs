//! What C3 added to Work, drawn: per-file counts, the verification figures,
//! a mission's due time, the note panel, the failing line.

use swarmux_application::console::{
    CheckRunView, CheckStatus, DiffFileView, DriftView, FirstRunView, MetricsView, SpreadView,
};

use super::*;
use crate::app::{InputPurpose, Mode};
use crate::ui::input::Input;
use crate::work::{NoteComposer, NoteTarget};

fn metrics() -> MetricsView {
    MetricsView {
        at: NOW,
        window_ms: 24 * 3_600_000,
        covered_ms: 24 * 3_600_000,
        complete: true,
        first_run: Some(FirstRunView {
            passed: 6,
            judged: 7,
            rate: 6.0 / 7.0,
        }),
        claim_to_verified: Some(SpreadView {
            median_ms: 130_000,
            p90_ms: 560_000,
            samples: 6,
        }),
        verified: 9,
        verified_per_hour: Some(0.375),
        checks_run: 41,
        bundles_sealed: 3,
    }
}

fn file(path: &str, ins: u32, del: u32) -> DiffFileView {
    DiffFileView {
        path: path.into(),
        insertions: ins,
        deletions: del,
        binary: false,
        change: Some(swarmux_application::console::ChangeKind::Modified),
    }
}

#[test]
fn the_diff_tab_lists_the_sealed_files_with_their_lines() {
    let snap = evening();
    let mut app = app();
    select(&mut app, &snap, "mission:fix-invoice");
    app.work_screen.show(Tab::Diff);
    let out = text(&draw(&app, &snap, 180, 50));
    for needle in [
        "sealed diff 3 files +20 -4",
        "FILES",
        "from the sealed bundle",
        "M src/etl.rs",
        "+12 -2",
        "M src/etl/counts.rs",
        "+5 -1",
        "M docs/etl.md",
        "+3 -1",
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
    // The counts sit beside the path, not at the far edge of the pane.
    let row = out.lines().find(|l| l.contains("M src/etl.rs")).unwrap();
    let right = row.split('│').nth(1).unwrap().trim_end();
    assert!(right.chars().count() < 45, "{right:?}");
}

#[test]
fn a_bundle_sealed_before_per_file_counts_shows_totals_only_and_says_so() {
    let mut snap = evening();
    for g in &mut snap.groups {
        for o in &mut g.outcomes {
            o.files.clear();
        }
    }
    let mut app = app();
    select(&mut app, &snap, "mission:fix-invoice");
    app.work_screen.show(Tab::Diff);
    let out = text(&draw(&app, &snap, 180, 50));
    assert!(out.contains("sealed diff 3 files +20 -4"), "{out}");
    assert!(out.contains("no per-file counts in this bundle"), "{out}");
    assert!(!out.contains("FILES"), "no count is invented\n{out}");
}

#[test]
fn a_binary_file_says_so_instead_of_showing_zeros() {
    let mut snap = evening();
    for g in &mut snap.groups {
        for o in &mut g.outcomes {
            if o.id == "mission:fix-invoice" {
                o.files.push(DiffFileView {
                    path: "assets/logo.png".into(),
                    insertions: 0,
                    deletions: 0,
                    binary: true,
                    change: Some(swarmux_application::console::ChangeKind::Added),
                });
            }
        }
    }
    let mut app = app();
    select(&mut app, &snap, "mission:fix-invoice");
    app.work_screen.show(Tab::Diff);
    let out = text(&draw(&app, &snap, 180, 50));
    let row = out
        .lines()
        .find(|l| l.contains("assets/logo.png"))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(
        row.contains("A assets/logo.png") && row.contains("binary"),
        "{row}"
    );
    assert!(!row.contains("+0"), "{row}");
}

#[test]
fn the_uncommitted_files_come_before_their_text() {
    let snap = evening();
    let mut app = app();
    select(&mut app, &snap, "job:3");
    app.work_screen.show(Tab::Diff);
    app.work_screen.diff = Some(crate::work::DiffRead {
        job: 3,
        files: vec![file("src/a.rs", 7, 1), file("src/b.rs", 0, 12)],
        lines: vec!["@@ -1 +1 @@".into(), "-old".into(), "+new".into()],
        note: None,
    });
    let out = text(&draw(&app, &snap, 180, 50));
    let at = |s: &str| out.find(s).unwrap_or_else(|| panic!("{s}\n{out}"));
    assert!(at("UNCOMMITTED CHANGES") < at("src/a.rs"), "{out}");
    assert!(at("src/b.rs") < at("@@ -1 +1 @@"), "{out}");
    assert!(out.contains("+7 -1") && out.contains("+0 -12"), "{out}");
}

#[test]
fn plan_against_drift_shows_what_each_task_took_and_what_fell_outside() {
    let mut snap = evening();
    let split = snap
        .groups
        .iter_mut()
        .flat_map(|g| g.outcomes.iter_mut())
        .find(|o| o.id == "mission:split-app")
        .unwrap();
    let plan = split.plan.as_mut().unwrap();
    plan.tasks[0].changed = vec![
        file("src/api/routes.rs", 30, 4),
        file("src/api/mod.rs", 2, 0),
    ];
    let drift: &mut DriftView = plan.drift.as_mut().unwrap();
    drift.outside_files = vec![file("scripts/x.sh", 7, 1)];
    let mut app = app();
    select(&mut app, &snap, "mission:split-app");
    let out = text(&draw(&app, &snap, 180, 50));
    for needle in [
        "src/api/routes.rs",
        "+30 -4",
        "src/api/mod.rs",
        "+2 -0",
        "scripts/x.sh",
        "+7 -1",
        "outside plan",
        // The planned task nothing touched says so under itself.
        "untouched: nothing changed in its areas",
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
}

#[test]
fn the_verification_figures_sit_under_the_capacity_and_leave_out_what_has_no_data() {
    let mut snap = evening();
    snap.metrics = Some(metrics());
    let app = app();
    let out = text(&draw(&app, &snap, 180, 50));
    for needle in [
        "last 24h first-run 86% (6/7) · 9 verified · 0.4/h",
        "claim→verified 2m10s med · 9m20s p90",
        "41 checks · 3 sealed",
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
    // No ledger read: no line at all.
    snap.metrics = None;
    let out = text(&draw(&app, &snap, 180, 50));
    assert!(!out.contains("last 24h"), "{out}");
    // Nothing judged: the line says so instead of showing zeros.
    let mut m = metrics();
    m.first_run = None;
    m.claim_to_verified = None;
    m.verified = 0;
    m.verified_per_hour = None;
    m.checks_run = 0;
    m.bundles_sealed = 0;
    snap.metrics = Some(m);
    let out = text(&draw(&app, &snap, 180, 50));
    assert!(out.contains("last 24h nothing judged yet"), "{out}");
    assert!(!out.contains("first-run"), "{out}");
    // A read cut short says the figures are the newest part only.
    let mut cut = metrics();
    cut.complete = false;
    snap.metrics = Some(cut);
    let out = text(&draw(&app, &snap, 180, 50));
    assert!(out.contains("newest only"), "{out}");
}

#[test]
fn a_due_time_shows_in_the_cost_line_only_when_the_contract_has_one() {
    let mut snap = evening();
    let plan_me = snap
        .groups
        .iter_mut()
        .flat_map(|g| g.outcomes.iter_mut())
        .find(|o| o.id == "mission:plan-me")
        .unwrap();
    // The demo clock is 1970-01-01 02:46:40 UTC (NOW = 10_000_000 ms).
    plan_me.contract.as_mut().unwrap().due = Some(NOW + 3 * 3_600_000 + 20 * 60_000);
    let app = app();
    let out = text(&draw(&app, &snap, 180, 50));
    assert!(
        out.lines()
            .any(|l| l.contains("cost ") && l.contains("due 06:06")),
        "{out}"
    );
    // Without a due time on any outcome, none is shown.
    let plain = evening();
    let out = text(&draw(&app, &plain, 180, 50));
    assert!(!out.contains("due "), "{out}");
}

#[test]
fn a_due_time_is_written_in_the_users_clock_and_says_when_it_is_late() {
    use super::super::util::due_text;
    let now = 1_793_613_600_000; // 2026-11-02 10:00 UTC
    // Today, a quarter past two hours ahead, in UTC and in +03:30.
    assert_eq!(due_text(now + 2 * 3_600_000, now, 0), "due 12:00");
    assert_eq!(due_text(now + 2 * 3_600_000, now, 210), "due 15:30");
    // Another day gets its date; the user's day decides what is another.
    assert_eq!(
        due_text(now + 20 * 3_600_000, now, 0),
        "due 2026-11-03 06:00"
    );
    assert_eq!(
        due_text(now + 14 * 3_600_000, now, 0),
        "due 2026-11-03 00:00"
    );
    // At UTC-10 it is still the same morning: 00:00 UTC on the 3rd is 14:00 on the 2nd.
    assert_eq!(due_text(now + 14 * 3_600_000, now, -600), "due 14:00");
    // Late.
    assert_eq!(
        due_text(now - 12 * 60_000, now, 0),
        "due 09:48 · overdue 12m"
    );
}

#[test]
fn the_header_names_the_due_time_too() {
    let mut snap = evening();
    let m = snap
        .groups
        .iter_mut()
        .flat_map(|g| g.outcomes.iter_mut())
        .find(|o| o.id == "mission:fix-invoice")
        .unwrap();
    m.contract.as_mut().unwrap().due = Some(NOW + 3_600_000);
    let mut app = app();
    select(&mut app, &snap, "mission:fix-invoice");
    let out = text(&draw(&app, &snap, 180, 50));
    assert!(out.contains("due 03:46"), "{out}");
}

#[test]
fn the_note_panel_says_where_the_note_goes_and_what_happens_next() {
    let snap = evening();
    let mut app = app();
    select(&mut app, &snap, "job:3");
    let suggestions = vec!["Run the linter.".to_string(), "Cut the scope.".to_string()];
    let mut composer = NoteComposer::new(NoteTarget::Work(3), "job #3", suggestions.clone());
    composer.cycle(1);
    app.work_screen.note = Some(composer);
    app.mode = Mode::Input(
        InputPurpose::SendBackNote,
        Input::with_text("Run the linter."),
    );
    let out = text(&draw(&app, &snap, 180, 50));
    for needle in [
        "SEND BACK",
        "job #3 resumes in its own session with your note",
        "the checks run again on its next claim",
        "note   Run the linter.",
        "enter sends it · esc drops it · tab picks a suggestion",
        "nothing is sent before you press enter",
        "Cut the scope.",
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
    // The actions panel gives way to it.
    assert!(!out.contains("ACTIONS"), "{out}");

    // For a held call the heading says it is a denial.
    let mut held = NoteComposer::new(NoteTarget::Held(7), "Bash git push", Vec::new());
    held.cycle(1);
    app.work_screen.note = Some(held);
    app.mode = Mode::Input(InputPurpose::SendBackNote, Input::default());
    select(&mut app, &snap, "job:2");
    let out = text(&draw(&app, &snap, 180, 50));
    assert!(out.contains("DENY WITH A NOTE"), "{out}");
    assert!(out.contains("Bash git push is denied; your note is the reason the agent reads"));
    assert!(out.contains("type below"), "{out}");
    // No suggestions: no tab hint.
    assert!(!out.contains("tab picks"), "{out}");
}

#[test]
fn a_suggestion_is_marked_only_while_the_line_still_holds_it() {
    let snap = evening();
    let mut app = app();
    select(&mut app, &snap, "job:3");
    let mut composer = NoteComposer::new(
        NoteTarget::Work(3),
        "job #3",
        vec!["Run the linter.".to_string()],
    );
    composer.cycle(1);
    app.work_screen.note = Some(composer);
    app.mode = Mode::Input(
        InputPurpose::SendBackNote,
        Input::with_text("Run the linter."),
    );
    let buf = draw(&app, &snap, 180, 50);
    // The key cell of the suggestion's row: the "1" right before its text.
    let marked = |buf: &Buffer| {
        let t = &app.look.theme;
        let want: Vec<&str> = "1  Run the linter."
            .split("")
            .filter(|s| !s.is_empty())
            .collect();
        (0..buf.area.height).any(|y| {
            (0..buf.area.width.saturating_sub(want.len() as u16)).any(|x| {
                let same = want
                    .iter()
                    .enumerate()
                    .all(|(i, s)| buf[(x + i as u16, y)].symbol() == *s);
                same && buf[(x, y)].bg == t.warn
            })
        })
    };
    assert!(marked(&buf), "picked and held: marked");
    // Edited: no longer the suggestion.
    app.mode = Mode::Input(
        InputPurpose::SendBackNote,
        Input::with_text("Run the linter, then the tests."),
    );
    let buf = draw(&app, &snap, 180, 50);
    assert!(!marked(&buf), "edited: not marked");
}

#[test]
fn the_failing_line_of_a_failed_check_shows_in_both_places() {
    let mut snap = evening();
    let o = snap
        .groups
        .iter_mut()
        .flat_map(|g| g.outcomes.iter_mut())
        .find(|o| o.id == "job:3")
        .unwrap();
    o.checks.runs = vec![
        CheckRunView {
            name: "cargo test".into(),
            status: CheckStatus::Failed,
            duration_ms: 41_000,
            first_failure: Some("thread 'etl::counts' panicked at src/etl.rs:12".into()),
        },
        CheckRunView {
            name: "cargo clippy".into(),
            status: CheckStatus::Error,
            duration_ms: 0,
            first_failure: Some("cannot start cargo: No such file or directory".into()),
        },
        CheckRunView {
            name: "cargo fmt".into(),
            status: CheckStatus::Passed,
            duration_ms: 900,
            first_failure: None,
        },
    ];
    o.checks.total = 3;
    o.checks.passed = 1;
    o.checks.failed = 2;
    let mut app = app();
    select(&mut app, &snap, "job:3");
    // The work product tab (claim against checks), and the checks tab.
    for tab in [Tab::Product, Tab::Checks] {
        app.work_screen.show(tab);
        let out = text(&draw(&app, &snap, 180, 50));
        for needle in [
            "thread 'etl::counts' panicked at src/etl.rs:12",
            "cannot start cargo: No such file or directory",
            "FAIL",
            "ERROR",
        ] {
            assert!(out.contains(needle), "{tab:?} {needle}\n{out}");
        }
        // A check that passed has no failing line.
        let fmt = out.lines().position(|l| l.contains("cargo fmt")).unwrap();
        assert!(
            !out.lines().nth(fmt + 1).unwrap_or("").contains("panicked"),
            "{out}"
        );
    }
}
