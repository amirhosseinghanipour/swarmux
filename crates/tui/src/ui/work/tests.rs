//! The Work screen drawn: at 180×50 and 120×36, with a decision selected,
//! a Guard hold, work ready to land in manual mode, and an action that is
//! disabled with its reason. Over the read model's own evening snapshot.

use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
use swarmux_application::console::{
    ConsoleSnapshot, GroupKind, LandingMode, LandingRepoView, LandingView, Lifecycle,
};

use super::render_with;
use crate::app::App;
use crate::app::tests_support::test_app;
use crate::work::Tab;
use crate::work::tests::{evening, outcome};

/// The demo clock the evening snapshot was taken at.
const NOW: i64 = 10_000_000;

fn app() -> App {
    let t = test_app();
    t.clock.advance(NOW - 1_000);
    t.app
}

/// The body of the Work screen at `w`×`h`, as the buffer a terminal holds.
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

fn select(app: &mut App, snap: &ConsoleSnapshot, id: &str) {
    assert!(app.work_screen.select(snap, id), "{id} is on the queue");
}

/// The delivered mission turned into one that waits to land: the gate
/// passed, nothing sealed, and `landing = "manual"` offers landing.
fn ready_in_manual_mode() -> ConsoleSnapshot {
    let mut snap = evening();
    let mut o = {
        let landed = snap
            .groups
            .iter_mut()
            .find(|g| g.kind == GroupKind::Landed)
            .unwrap();
        let at = landed
            .outcomes
            .iter()
            .position(|o| o.id == "mission:fix-invoice")
            .unwrap();
        landed.total -= 1;
        landed.outcomes.remove(at)
    };
    o.group = GroupKind::ReadyToLand;
    o.lifecycle = Lifecycle::Verifying;
    o.evidence = None;
    o.diff = None;
    o.landing = Some(LandingView {
        mode: LandingMode::Preview,
        into: "swarmux/fix-invoice/integration".into(),
        repos: ["api-gateway", "billing"]
            .into_iter()
            .map(|r| LandingRepoView {
                repo: r.into(),
                branch: None,
                state: swarmux_application::console::LandState::Waiting,
                commit: None,
                note: None,
            })
            .collect(),
        queue: Vec::new(),
        complete: false,
        summary:
            "2 repositories land in swarmux/fix-invoice/integration, producers first, all or none"
                .into(),
        at: None,
    });
    for a in &mut o.actions {
        match a.kind {
            swarmux_application::console::ActionKind::LandToIntegration => {
                a.enabled = true;
                a.reason = None;
            }
            swarmux_application::console::ActionKind::OpenEvidence => {
                a.enabled = false;
                a.reason =
                    Some("no evidence bundle yet: one is sealed when the gate passes".into());
            }
            _ => {}
        }
    }
    let ready = snap
        .groups
        .iter_mut()
        .find(|g| g.kind == GroupKind::ReadyToLand)
        .unwrap();
    ready.total += 1;
    ready.outcomes.insert(0, o);
    snap.header.counts.landed -= 1;
    snap.header.counts.ready_to_land += 1;
    snap
}

/// Every line fits the screen, and nothing is drawn in a private-use glyph.
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

#[test]
fn a_decision_selected_shows_the_queue_the_hold_and_the_actions_at_180x50() {
    let snap = evening();
    let mut app = app();
    select(&mut app, &snap, "job:2");
    let buf = draw(&app, &snap, 180, 50);
    let out = text(&buf);
    assert_clean(&buf, "180x50");

    // The queue: the four groups with their counts, ranked, with the cost
    // line made of what exists.
    for needle in [
        "DECIDE 4",
        "only you can",
        "READY TO LAND 1",
        "RUNNING",
        "LANDED 1",
        "P1",
        "P2",
        "auto-deny in",
        "CAPACITY",
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
    // The selected outcome: header, tabs, Guard hold, the agent's words.
    for needle in [
        "job 2: web-checkout",
        "HELD BY GUARD",
        "1 work product",
        "2 stream",
        "3 diff",
        "4 checks",
        "THE CALL GUARD HELD",
        "Bash git push origin HEAD:main",
        "git push asks",
        "destructive",
        "AGENT'S OWN WORDS",
        "Pushing the branch with the test as it was.",
        "LAST STEPS",
        "ACTIONS",
        "allow once",
        "always allow",
        "deny",
        "THE CALL GUARD REFUSED",
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
    // The agent's words are quoted and labelled, as its own.
    assert!(out.contains("“Pushing the branch"), "{out}");
    // The selected row of the queue is marked.
    assert!(out.contains("▌ ◆ job 2: web-checkout"), "{out}");
    // Nothing the product cannot do.
    for banned in ["send back", "pause", "merge to main", "mission due"] {
        assert!(!out.contains(banned), "{banned}");
    }
}

#[test]
fn the_same_decision_fits_120x36_without_losing_the_hold_or_its_actions() {
    let snap = evening();
    let mut app = app();
    select(&mut app, &snap, "job:2");
    let buf = draw(&app, &snap, 120, 36);
    let out = text(&buf);
    assert_clean(&buf, "120x36");
    assert_eq!(out.lines().count(), 36);
    for needle in [
        "DECIDE 4",
        "HELD BY GUARD",
        "THE CALL GUARD HELD",
        "Bash git push origin HEAD:main",
        "AGENT'S OWN WORDS",
        "ACTIONS",
        "allow once",
        "deny",
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
    // The queue does not push the pane out: its column stays a third.
    let widest = out.lines().map(|l| l.chars().count()).max().unwrap();
    assert!(widest <= 120, "{widest}");
}

#[test]
fn the_work_product_of_a_delivered_mission_shows_proof_and_what_landing_did() {
    let snap = evening();
    let mut app = app();
    select(&mut app, &snap, "mission:fix-invoice");
    let buf = draw(&app, &snap, 180, 50);
    let out = text(&buf);
    for needle in [
        "DELIVERED",
        "PLAN vs DRIFT",
        "no plan: it worked from the prompt",
        "CLAIM vs CHECKS",
        "agent's claim",
        "CHECKS",
        "cargo test",
        "pass",
        "EVIDENCE",
        "bundle",
        "ledger #",
        "signature",
        "ACTIONS",
        "evidence",
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
    // The disabled land action says why.
    assert!(out.contains("land"), "{out}");
    assert!(out.contains("the work is delivered"), "{out}");
}

#[test]
fn work_ready_to_land_in_manual_mode_shows_what_landing_does_and_offers_it() {
    let snap = ready_in_manual_mode();
    let mut app = app();
    select(&mut app, &snap, "mission:fix-invoice");
    for (w, h) in [(180, 50), (120, 36)] {
        let buf = draw(&app, &snap, w, h);
        let out = text(&buf);
        assert_clean(&buf, &format!("{w}x{h}"));
        for needle in [
            "READY TO LAND 2",
            "VERIFIED",
            "WHAT LANDING DOES",
            "swarmux/fix-invoice/integration",
            "producers first: api-gateway → billing",
            "all or none",
            "untouched",
            "ACTIONS",
            "land",
        ] {
            assert!(out.contains(needle), "{w}x{h}: {needle}\n{out}");
        }
        // Nothing is sealed until it lands, and it says so.
        assert!(out.contains("sealed on delivery"), "{out}");
        // The land key is the keymap's, shown on its row.
        let key = app.key_of(crate::screens::work::LAND).unwrap();
        assert!(
            out.lines()
                .any(|l| l.contains(&format!(" {key} ")) && l.contains("land")),
            "{key}\n{out}"
        );
    }
}

#[test]
fn a_disabled_action_shows_dim_with_the_reason_in_its_place() {
    let snap = evening();
    let mut app = app();
    select(&mut app, &snap, "mission:plan-me");
    let buf = draw(&app, &snap, 180, 50);
    let out = text(&buf);
    assert!(out.contains("approve plan"), "{out}");
    assert!(out.contains("change plan"), "{out}");
    let (x, y) = (0..buf.area.height)
        .find_map(|y| {
            let row: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
            row.find("land").and_then(|_| {
                row.contains("landing on request is for workspace missions")
                    .then_some((0u16, y))
            })
        })
        .expect("the land row says why it cannot");
    // Dim, like every disabled row: the text role, not the live one.
    let t = &app.look.theme;
    let label = (x..buf.area.width)
        .find(|x| buf[(*x, y)].symbol() == "l" && buf[(x + 1, y)].symbol() == "a")
        .unwrap();
    assert_eq!(buf[(label, y)].fg, t.dim);
}

#[test]
fn the_tabs_show_the_stream_the_diff_and_every_check() {
    let snap = evening();
    let mut app = app();
    select(&mut app, &snap, "job:3");

    app.work_screen.show(Tab::Checks);
    let out = text(&draw(&app, &snap, 180, 50));
    for needle in [
        "CHECKS",
        "cargo test",
        "FAIL",
        "thread 'etl::counts' panicked at src/etl.rs:12",
        "protected pins",
        "test-weakening",
        "base suite",
        "hidden checks",
        "evidence gaps",
        "drift",
        "reviewer",
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }

    app.work_screen.show(Tab::Stream);
    let out = text(&draw(&app, &snap, 180, 50));
    assert!(out.contains("ran before this session"), "{out}");

    app.work_screen.show(Tab::Diff);
    let out = text(&draw(&app, &snap, 180, 50));
    assert!(out.contains("the sealed diff totals appear when"), "{out}");
    app.work_screen.diff = Some(crate::work::DiffRead {
        job: 3,
        lines: vec!["@@ -1 +1 @@".into(), "-old".into(), "+new".into()],
        note: None,
    });
    let buf = draw(&app, &snap, 180, 50);
    let out = text(&buf);
    assert!(out.contains("UNCOMMITTED CHANGES"), "{out}");
    assert!(out.contains("+new"), "{out}");
}

#[test]
fn an_empty_queue_says_so_and_a_small_window_asks_for_room() {
    let mut snap = evening();
    for g in &mut snap.groups {
        g.outcomes.clear();
        g.total = 0;
    }
    snap.holds.clear();
    let app = app();
    let out = text(&draw(&app, &snap, 120, 36));
    assert!(out.contains("nothing needs you"), "{out}");
    assert!(out.contains("nothing selected"), "{out}");
    let out = text(&draw(&app, &evening(), 70, 12));
    assert!(out.contains("make the window bigger"), "{out}");
}

#[test]
fn every_outcome_draws_at_both_sizes_within_the_screen() {
    let snap = evening();
    let mut app = app();
    let all: Vec<String> = crate::work::flat(&snap)
        .iter()
        .map(|o| o.id.clone())
        .collect();
    for id in &all {
        for tab in Tab::ALL {
            app.work_screen.select(&snap, id);
            app.work_screen.show(tab);
            for (w, h) in [(180, 50), (120, 36), (100, 30)] {
                let buf = draw(&app, &snap, w, h);
                assert_clean(&buf, &format!("{id} {tab:?} {w}x{h}"));
                assert!(
                    text(&buf)
                        .lines()
                        .all(|l| l.chars().count() <= usize::from(w)),
                    "{id} {tab:?} {w}x{h}"
                );
            }
        }
    }
    let _ = outcome(&snap, "job:2");
}

#[test]
fn the_cost_line_only_counts_what_the_plans_task_dependencies_hold_up() {
    let mut snap = evening();
    let mut app = app();
    select(&mut app, &snap, "job:2");
    // In the evening the push only shares files with the queued job: that
    // is a relation (the header says so), not a cost of delay.
    let out = text(&draw(&app, &snap, 180, 50));
    assert!(out.contains("auto-deny in"), "{out}");
    let cost: Vec<&str> = out.lines().filter(|l| l.contains("cost ")).collect();
    assert!(!cost.is_empty(), "{out}");
    assert!(cost.iter().all(|l| !l.contains("blocks")), "{cost:?}");
    assert!(out.contains("graph blocks job 4"), "{out}");

    // A task dependency on the push is.
    for e in &mut snap.deck.edges {
        if e.kind == swarmux_application::console::EdgeKind::FileClaim {
            e.kind = swarmux_application::console::EdgeKind::TaskDependency;
        }
    }
    let out = text(&draw(&app, &snap, 180, 50));
    assert!(
        out.lines()
            .any(|l| l.contains("cost ") && l.contains("blocks job 4: web-checkout")),
        "{out}"
    );
}

#[test]
fn the_capacity_footer_reads_the_windows_the_snapshot_carries() {
    let mut snap = evening();
    let now = NOW;
    snap.header.limits = vec![
        swarmux_application::console::LimitWindow {
            window: "five_hour".into(),
            status: "allowed".into(),
            utilization: Some(0.63),
            resets_at: Some(now + 2 * 3_600_000 + 13 * 60_000),
            hits_at: Some(now + 2 * 3_600_000 + 3 * 60_000),
            stale: false,
        },
        swarmux_application::console::LimitWindow {
            window: "seven_day".into(),
            status: "allowed".into(),
            utilization: Some(0.41),
            resets_at: None,
            hits_at: None,
            stale: false,
        },
    ];
    let app = app();
    let out = text(&draw(&app, &snap, 180, 50));
    assert!(out.contains("5h"), "{out}");
    assert!(out.contains("63%"), "{out}");
    assert!(out.contains("cap in 2h03"), "{out}");
    assert!(out.contains("reset 2h13"), "{out}");
    assert!(out.contains("7d"), "{out}");
    assert!(out.contains("41%"), "{out}");
    assert!(out.contains("slots 1/4 · queued 1"), "{out}");
}

#[test]
fn the_land_action_says_what_it_does_where_there_is_room() {
    let snap = ready_in_manual_mode();
    let mut app = app();
    select(&mut app, &snap, "mission:fix-invoice");
    let out = text(&draw(&app, &snap, 180, 50));
    assert!(
        out.contains(
            "to swarmux/fix-invoice/integration · producers first, all or none, undo if one fails"
        ),
        "{out}"
    );
}

/// A buffer as `tmux capture-pane -p -e` writes it: SGR colour runs, one
/// line per row. What the tmux-to-PNG renderer reads.
fn ansi(buf: &Buffer) -> String {
    use ratatui::style::{Color, Modifier};
    let rgb = |c: Color| match c {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    };
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            out.push_str("\x1b[0m");
            if cell.modifier.contains(Modifier::BOLD) {
                out.push_str("\x1b[1m");
            }
            if let Some((r, g, b)) = rgb(cell.fg) {
                out.push_str(&format!("\x1b[38;2;{r};{g};{b}m"));
            }
            if let Some((r, g, b)) = rgb(cell.bg) {
                out.push_str(&format!("\x1b[48;2;{r};{g};{b}m"));
            }
            out.push_str(cell.symbol());
        }
        out.push_str("\x1b[0m\n");
    }
    out
}

/// Writes the states the owner reviews, drawn by the real renderer over the
/// read model's evening snapshot: work ready to land in manual mode, and an
/// escalated outcome. `SWARMUX_SHOT_DIR=out cargo test -p swarmux-tui
/// write_review_screens -- --ignored`.
#[test]
#[ignore = "writes review screens when SWARMUX_SHOT_DIR is set"]
fn write_review_screens() {
    let Ok(dir) = std::env::var("SWARMUX_SHOT_DIR") else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = app();
    let manual = ready_in_manual_mode();
    select(&mut app, &manual, "mission:fix-invoice");
    for (w, h) in [(180, 47), (120, 33)] {
        let buf = draw(&app, &manual, w, h);
        std::fs::write(format!("{dir}/ready_manual_{w}.ans"), ansi(&buf)).unwrap();
    }
    let evening = evening();
    select(&mut app, &evening, "job:3");
    app.work_screen.show(Tab::Product);
    let buf = draw(&app, &evening, 180, 47);
    std::fs::write(format!("{dir}/escalated_180.ans"), ansi(&buf)).unwrap();
}

#[test]
fn a_run_of_held_calls_shows_the_first_few_and_the_agents_words_once() {
    let mut snap = evening();
    let held = snap.holds[0].clone();
    for n in 0..6u64 {
        let mut h = held.clone();
        h.id = format!("hold:{}", 100 + n);
        h.request = Some(100 + n);
        h.target = format!("cargo check -p docs-{n}");
        h.at = held.at - i64::try_from(n).unwrap();
        snap.holds.push(h);
    }
    let mut app = app();
    select(&mut app, &snap, "job:2");
    let out = text(&draw(&app, &snap, 180, 60));
    assert!(out.contains("more calls held or refused"), "{out}");
    // Three calls in full, not all seven.
    assert_eq!(out.matches("THE CALL GUARD HELD").count(), 3, "{out}");
    // The words the agent said once, however many calls share them.
    assert_eq!(out.matches("“Pushing the branch").count(), 1, "{out}");
}
