use crossterm::event::KeyCode;
use ratatui::style::Color;
use swarmux_application::GateState;
use swarmux_application::ports::RunnerMsg;
use swarmux_domain::gate::GateOutcome;
use swarmux_domain::{Actor, AgentEvent};

use crate::app::Screen;
use crate::app::tests_support::TestApp;
use crate::ui::dashboard::tests::{reviewed_job, running_job};
use crate::ui::look::IconSet;
use crate::ui::mission::tests::scenario;
use crate::ui::{render_buffer, render_to_string};

fn on_focus(mut t: TestApp) -> TestApp {
    t.app.dashboard.classic = false;
    t.app.screen = Screen::Dashboard;
    t.app.update();
    t
}

fn gate(t: &mut TestApp, outcome: GateOutcome) {
    let job = t.app.current_job().unwrap().job_id;
    t.app.mission.gates.insert(
        job,
        GateState {
            outcome,
            blocks: 1,
            max_blocks: 3,
            checks: vec![
                ("build".into(), true, false),
                ("tests 28/28".into(), outcome == GateOutcome::Pass, false),
                ("lint".into(), true, true),
            ],
            protected_changed: 0,
            evidence_gaps: usize::from(outcome != GateOutcome::Pass),
            digest: String::new(),
            problem: None,
            at: 0,
        },
    );
}

fn emit_text(t: &TestApp, text: &str) {
    t.runner.emit(
        0,
        RunnerMsg::Event(AgentEvent::Text {
            text: text.into(),
            actor: Actor::Lead,
        }),
    );
}

#[test]
fn snapshot_160x48_standard() {
    let mut t = reviewed_job();
    gate(&mut t, GateOutcome::Pass);
    t.app.look.icons = IconSet::Unicode;
    let t = on_focus(t);
    let out = render_to_string(&t.app, 160, 48);
    println!("{out}");
    for needle in [
        "demo-shop",
        "situation",
        "doing",
        "because",
        "next",
        "sure",
        "unsure",
        "gate",
        "build",
        "tests 28/28",
        "lint flaky",
        "review pass",
        "checkpoints none yet",
        "recovery nothing needed",
        "timeline",
        "you",
        "action stream",
        "Task",
        "answer",
        "Added GET /health",
        "reviewer · Opus 5.5",
    ] {
        assert!(out.contains(needle), "missing {needle:?}\n{out}");
    }
    assert!(!out.contains("make the window bigger"));
    assert!(!out.contains("tail -f session.log"), "the old grid is gone");
}

#[test]
fn snapshot_120x36_compact_and_240x64_full() {
    let mut t = reviewed_job();
    gate(&mut t, GateOutcome::Pass);
    let t = on_focus(t);
    let compact = render_to_string(&t.app, 120, 36);
    for needle in ["situation", "action stream", "answer", "reviewer ·"] {
        assert!(
            compact.contains(needle),
            "compact missing {needle:?}\n{compact}"
        );
    }
    assert!(!compact.contains(" plan "), "plan is for the full density");
    let full = render_to_string(&t.app, 240, 64);
    for needle in [
        "situation",
        "action stream",
        "answer",
        "reviewer ·",
        " plan ",
        "agents",
    ] {
        assert!(full.contains(needle), "full missing {needle:?}\n{full}");
    }
    assert!(full.contains("worker") && full.contains("scout"));
}

#[test]
fn the_reviewer_column_only_shows_when_a_review_was_used() {
    let mut t = crate::app::tests_support::test_app();
    t.with_session("plain");
    t.send("say hi");
    t.app.update();
    let t = on_focus(t);
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains("action stream"));
    assert!(!out.contains("reviewer"), "no review, no column\n{out}");
    let used = on_focus(reviewed_job());
    assert!(render_to_string(&used.app, 160, 48).contains("reviewer"));
}

#[test]
fn enter_on_a_card_lands_on_its_job_with_the_same_sigil_and_hue() {
    let mut t = scenario();
    t.app.dashboard.classic = false;
    t.app.look.icons = IconSet::Unicode;
    t.keys([KeyCode::Char('1'), KeyCode::Tab]);
    // Walk the wall to "api" (second tile, a Sonnet job).
    t.keys([KeyCode::Down, KeyCode::Enter]);
    assert_eq!(t.app.screen, Screen::Dashboard);
    assert_eq!(t.app.current_job().unwrap().session_name, "api");
    t.app.update();

    // The sigil on its card ...
    t.keys([KeyCode::Char('1')]);
    let wall = render_buffer(&t.app, 160, 48);
    let card = find_sigil(&wall, 'A').expect("the api card has a sigil");
    // ... and in the Focus header, once the arrival highlight has settled.
    t.keys([KeyCode::Char('2')]);
    t.app.frame += 100;
    let focus = render_buffer(&t.app, 160, 48);
    let head = find_sigil(&focus, 'A').expect("the header has the sigil");
    assert_eq!(card.0, head.0, "same hue on the card and in the header");
}

/// The colour of the first bold single-letter sigil `c` that has a space on
/// each side, as `(fg, y)`.
fn find_sigil(buf: &ratatui::buffer::Buffer, c: char) -> Option<(Color, u16)> {
    let area = buf.area;
    for y in area.y + 2..area.bottom() {
        for x in area.x + 1..area.right().saturating_sub(1) {
            let cell = &buf[(x, y)];
            if cell.symbol() == c.to_string()
                && buf[(x - 1, y)].symbol() == " "
                && buf[(x + 1, y)].symbol() == " "
                && cell.modifier.contains(ratatui::style::Modifier::BOLD)
            {
                return Some((cell.fg, y));
            }
        }
    }
    None
}

#[test]
fn enter_on_an_inbox_item_opens_its_job() {
    let mut t = scenario();
    t.app.dashboard.classic = false;
    t.keys([KeyCode::Char('1'), KeyCode::Enter]);
    assert_eq!(t.app.screen, Screen::Dashboard);
    let job = t.app.current_job().unwrap();
    assert!(job.session_name == "demo-shop" || job.session_name == "api");
}

#[test]
fn the_wall_selection_moves_with_the_arrows_and_never_leaves_the_wall() {
    let mut t = scenario();
    t.keys([KeyCode::Char('1'), KeyCode::Tab]);
    t.keys([
        KeyCode::Down,
        KeyCode::Down,
        KeyCode::Down,
        KeyCode::Down,
        KeyCode::Down,
    ]);
    assert_eq!(t.app.mission.wall_at, 3);
    t.keys([KeyCode::Up; 9]);
    assert_eq!(t.app.mission.wall_at, 0);
    // Tab hands the arrows back to the inbox.
    t.keys([KeyCode::Tab, KeyCode::Down]);
    assert_eq!(t.app.mission.wall_at, 0);
}

#[test]
fn t_opens_the_session_in_a_terminal_and_does_not_change_the_theme() {
    let mut t = on_focus(reviewed_job());
    let theme = t.app.look.theme.name;
    t.keys([KeyCode::Char('t')]);
    assert_eq!(t.app.look.theme.name, theme);
    let opened = t.terminal.opened();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].0, std::path::PathBuf::from("/work"));
    let session = t.app.selected_session().unwrap().id.to_string();
    assert_eq!(opened[0].1, session);
    assert!(t.app.notice.as_deref().unwrap().contains("demo-shop"));
}

#[test]
fn a_terminal_that_fails_to_open_is_reported() {
    let mut t = on_focus(reviewed_job());
    t.terminal.fail_next();
    t.keys([KeyCode::Char('t')]);
    assert!(t.app.error.as_deref().unwrap().contains("no terminal"));
}

#[test]
fn t_still_cycles_the_theme_elsewhere() {
    let mut t = crate::app::tests_support::test_app();
    let before = t.app.look.theme.name;
    t.keys([KeyCode::Char('t')]);
    assert_ne!(t.app.look.theme.name, before);
}

#[test]
fn human_events_are_marked_on_the_timeline() {
    let mut t = on_focus(reviewed_job());
    t.app.look.icons = IconSet::Unicode;
    let out = render_to_string(&t.app, 160, 48);
    let you = out
        .lines()
        .find(|l| l.contains("│you "))
        .expect("a you lane");
    assert!(you.contains('▲'), "the prompt and the two approvals: {you}");
    assert!(you.matches('▲').count() >= 2, "{you}");
}

#[test]
fn you_were_here_marks_what_happened_while_away() {
    let mut t = on_focus(running_job());
    t.app.look.icons = IconSet::Unicode;
    t.keys([KeyCode::Char('1')]);
    t.app.update();
    for i in 0..4 {
        emit_text(&t, &format!("progress note {i}"));
    }
    t.app.update();
    t.keys([KeyCode::Char('2')]);
    t.app.update();
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains("you were here"), "{out}");
    assert!(out.contains("4 new"), "{out}");
    let here = out
        .lines()
        .position(|l| l.contains("you were here"))
        .unwrap();
    let note = out
        .lines()
        .position(|l| l.contains("progress note 0"))
        .unwrap();
    assert!(here < note, "the marker comes before what is new");
}

#[test]
fn no_marker_on_the_first_visit() {
    let t = on_focus(running_job());
    assert!(!render_to_string(&t.app, 160, 48).contains("you were here"));
}

#[test]
fn the_stream_scrolls_back_and_follows_again() {
    let mut t = on_focus(running_job());
    for i in 0..80 {
        emit_text(&t, &format!("note number {i:02}"));
    }
    t.app.update();
    let tail = render_to_string(&t.app, 120, 36);
    assert!(tail.contains("note number 79"));
    t.keys([KeyCode::PageUp, KeyCode::PageUp]);
    let back = render_to_string(&t.app, 120, 36);
    assert!(!back.contains("note number 79"), "{back}");
    assert!(back.contains("back"));
    t.keys([
        KeyCode::Down,
        KeyCode::PageDown,
        KeyCode::PageDown,
        KeyCode::Down,
    ]);
    assert!(render_to_string(&t.app, 120, 36).contains("note number 79"));
    t.keys([KeyCode::Home]);
    assert!(
        render_to_string(&t.app, 120, 36).contains("note number 00")
            || t.app.focus_state.scroll > 0
    );
    t.keys([KeyCode::End]);
    assert_eq!(t.app.focus_state.scroll, 0);
}

#[test]
fn diffs_in_the_stream_are_tinted_and_keep_their_sign() {
    let mut t = on_focus(running_job());
    emit_text(&t, "+let health = true;");
    emit_text(&t, "-let health = false;");
    emit_text(&t, "- a bullet is prose");
    t.app.update();
    let buf = render_buffer(&t.app, 160, 48);
    let out = crate::ui::buffer_to_string(&buf);
    let theme = &t.app.look.theme;
    let find = |needle: &str| {
        let (y, line) = out
            .lines()
            .enumerate()
            .find(|(_, l)| l.contains(needle))
            .unwrap_or_else(|| panic!("no {needle}\n{out}"));
        let x = line[..line.find(needle).unwrap()].chars().count();
        buf[(x as u16, y as u16)].clone()
    };
    let added = find("+let health = true;");
    let removed = find("-let health = false;");
    let bullet = find("- a bullet is prose");
    assert_eq!(added.fg, theme.ok);
    assert_eq!(removed.fg, theme.error);
    assert_ne!(added.bg, theme.panel_bg, "a wash of green behind it");
    assert_ne!(bullet.fg, theme.ok);
    assert_ne!(bullet.fg, theme.error);
}

#[test]
fn d_swaps_the_answer_for_the_working_tree_diff() {
    let mut t = on_focus(reviewed_job());
    t.keys([KeyCode::Char('d')]);
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains("working tree diff"), "{out}");
    assert!(out.contains("+diff"));
    t.keys([KeyCode::Char('d')]);
    assert!(render_to_string(&t.app, 160, 48).contains("Added GET /health"));
}

#[test]
fn focus_is_the_default_view() {
    assert!(!crate::dashboard::DashboardState::default().classic);
}

#[test]
fn the_classic_grid_is_one_key_away() {
    let mut t = on_focus(reviewed_job());
    t.keys([KeyCode::Char('D')]);
    assert!(render_to_string(&t.app, 160, 48).contains("tail -f session.log"));
    t.keys([KeyCode::Char('D')]);
    assert!(render_to_string(&t.app, 160, 48).contains("action stream"));
}

#[test]
fn scrubbing_without_checkpoints_says_so() {
    let mut t = on_focus(reviewed_job());
    t.keys([KeyCode::Char(']')]);
    assert_eq!(t.app.focus_state.checkpoint, None);
    assert!(t.app.notice.as_deref().unwrap().contains("no checkpoints"));
}

#[test]
fn a_blocked_gate_lowers_the_confidence_and_says_why() {
    let mut t = reviewed_job();
    gate(&mut t, GateOutcome::Block);
    let t = on_focus(t);
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains("confidence low"), "{out}");
    assert!(out.contains("gate blocked 1/3"));
    assert!(out.contains("evidence missing"));
}

#[test]
fn a_replayed_job_from_history_shows_its_answer() {
    let mut t = crate::app::tests_support::test_app();
    t.with_session("old");
    t.send("old work");
    let job = t.app.current_job().unwrap().job_id;
    let id = t.runner.request(0).session_id.to_string();
    t.runner.emit(
        0,
        RunnerMsg::Event(AgentEvent::Init {
            session_id: id,
            model: "claude-sonnet-4-5".into(),
            cwd: "/work".into(),
            permission_mode: "default".into(),
        }),
    );
    t.runner.emit(
        0,
        RunnerMsg::Event(AgentEvent::Result {
            is_error: false,
            subtype: "success".into(),
            cost_usd: 0.1,
            duration_ms: 1000,
            num_turns: 1,
            tokens_in: 1,
            tokens_out: 1,
            cache_read: 0,
            context_window: Some(200_000),
            subagents_spawned: 0,
            text: "# Report\nAll good.".into(),
        }),
    );
    t.app.update();
    let on = on_focus(t);
    let out = render_to_string(&on.app, 160, 48);
    assert!(out.contains("Report") && out.contains("All good."), "{out}");
    let _ = job;
}

#[test]
fn every_size_and_icon_tier_renders_without_panicking() {
    let mut t = reviewed_job();
    gate(&mut t, GateOutcome::Block);
    let mut t = on_focus(t);
    for icons in [IconSet::Ascii, IconSet::Unicode, IconSet::Nerd] {
        t.app.look.icons = icons;
        for w in [0, 1, 30, 59, 60, 80, 99, 100, 120, 140, 160, 220, 240, 300] {
            for h in [0, 5, 13, 14, 24, 36, 40, 48, 60, 64, 80] {
                let _ = render_to_string(&t.app, w, h);
            }
        }
    }
    // Scrolled far past the end, with the diff open.
    t.keys([KeyCode::Char('d'), KeyCode::Home, KeyCode::Char('J')]);
    let _ = render_to_string(&t.app, 120, 36);
}

#[test]
fn focus_without_any_job_asks_for_one() {
    let mut t = crate::app::tests_support::test_app();
    t.app.dashboard.classic = false;
    t.with_session("fresh");
    t.app.screen = Screen::Dashboard;
    let out = render_to_string(&t.app, 120, 36);
    assert!(out.contains("nothing to focus on yet"), "{out}");
    let mut empty = crate::app::tests_support::test_app();
    empty.app.dashboard.classic = false;
    empty.app.screen = Screen::Dashboard;
    assert!(render_to_string(&empty.app, 120, 36).contains("press 6, then n"));
}

#[test]
fn only_one_thing_moves_on_focus() {
    // Two frames apart the screen is identical except for the spinner.
    let mut t = on_focus(running_job());
    t.app.look.icons = IconSet::Unicode;
    t.app.frame = 40;
    let a = render_buffer(&t.app, 160, 48);
    t.app.frame = 43;
    let b = render_buffer(&t.app, 160, 48);
    let moved: Vec<_> = a
        .content()
        .iter()
        .zip(b.content())
        .filter(|(x, y)| x != y)
        .collect();
    assert!(
        moved.len() <= 2,
        "{} cells changed between frames",
        moved.len()
    );
}

/// Render cost, printed with `--ignored --nocapture`.
#[test]
#[ignore = "timing"]
fn render_cost() {
    let mut t = reviewed_job();
    gate(&mut t, GateOutcome::Pass);
    for i in 0..400 {
        emit_text(&t, &format!("note {i}"));
    }
    let t = on_focus(t);
    for (w, h) in [(120, 36), (160, 48), (240, 64)] {
        let _ = render_buffer(&t.app, w, h);
        let n = 200;
        let start = std::time::Instant::now();
        for _ in 0..n {
            let _ = render_buffer(&t.app, w, h);
        }
        println!("{w}x{h}: {:?} per frame", start.elapsed() / n);
    }
}
