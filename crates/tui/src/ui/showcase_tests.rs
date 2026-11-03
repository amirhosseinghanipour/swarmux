//! The Constellation and the Spectrum through the real app: keys, freeze
//! on P1/P2, snapshots at 160 x 48 and the idle CPU measurement.

use crossterm::event::KeyCode;
use ratatui::style::Color;
use swarmux_application::MotionSetting;
use swarmux_application::ports::RunnerMsg;
use swarmux_domain::{Actor, AgentEvent};

use super::{render_buffer, render_to_string};
use crate::app::Screen;
use crate::app::tests_support::{TestApp, test_app};
use crate::showcase::Mode;
use crate::ui::mission::tests::scenario;
use crate::ui::motion::Motion;

fn emit(t: &TestApp, i: usize, e: AgentEvent) {
    t.runner.emit(i, RunnerMsg::Event(e));
}

/// Three agents at work and nobody waiting on the user: no P1 or P2 item.
fn calm() -> TestApp {
    let mut t = test_app();
    for (i, (name, model)) in [
        ("demo-shop", "claude-opus-4-5"),
        ("api", "claude-sonnet-4-5"),
        ("docs", "claude-haiku-4-5"),
    ]
    .into_iter()
    .enumerate()
    {
        t.with_session(name);
        t.send("keep going");
        let id = t.runner.request(i).session_id.to_string();
        emit(
            &t,
            i,
            AgentEvent::Init {
                session_id: id,
                model: model.into(),
                cwd: "/work".into(),
                permission_mode: "acceptEdits".into(),
            },
        );
        emit(
            &t,
            i,
            AgentEvent::Usage {
                input: 800 * (i as u64 + 1),
                output: 300,
                cache_read: 40_000 * (i as u64 + 1),
                cache_creation: 900,
                actor: Actor::Lead,
            },
        );
        t.clock.advance(1_000);
    }
    for round in 0..14u64 {
        for i in 0..3usize {
            emit(
                &t,
                i,
                AgentEvent::Usage {
                    input: 300 + 150 * ((round * 5 + i as u64 * 3) % 9),
                    output: 100 + 40 * ((round * 3 + i as u64) % 7),
                    cache_read: 30_000 * (i as u64 + 1) + round * 4_000,
                    cache_creation: 400,
                    actor: Actor::Lead,
                },
            );
        }
        t.app.update();
        t.clock.advance(60_000);
    }
    for (n, name) in ["Read", "Edit", "Bash"].into_iter().enumerate() {
        emit(
            &t,
            0,
            AgentEvent::ToolUse {
                id: format!("t{n}"),
                name: name.into(),
                summary: "src/lib.rs".into(),
                actor: Actor::Lead,
            },
        );
    }
    t.app.update();
    t.app.screen = Screen::Mission;
    t
}

fn alerts(t: &TestApp) -> usize {
    t.app
        .mission
        .items
        .iter()
        .filter(|i| {
            matches!(
                i.priority,
                swarmux_domain::Priority::P1 | swarmux_domain::Priority::P2
            )
        })
        .count()
}

fn step(t: &mut TestApp, ms: i64) {
    t.clock.advance(ms);
    t.app.update();
}

#[test]
fn z_and_s_open_the_showcase_and_esc_leaves() {
    let mut t = calm();
    t.keys([KeyCode::Char('z')]);
    assert_eq!(t.app.showcase.mode, Some(Mode::Constellation));
    t.keys([KeyCode::Char('S')]);
    assert_eq!(t.app.showcase.mode, Some(Mode::Spectrum));
    t.keys([KeyCode::Char('z')]);
    assert_eq!(t.app.showcase.mode, Some(Mode::Constellation));
    t.keys([KeyCode::Esc]);
    assert_eq!(t.app.showcase.mode, None);
    t.keys([KeyCode::Char('S'), KeyCode::Char('S')]);
    assert_eq!(t.app.showcase.mode, None, "the same key leaves");
    t.keys([KeyCode::Char('z'), KeyCode::Char('4')]);
    assert_eq!(
        t.app.showcase.mode, None,
        "a screen key leaves and goes there"
    );
    assert_eq!(t.app.screen, Screen::Queue);
    t.keys([KeyCode::Char('z')]);
    assert_eq!(t.app.showcase.mode, None, "not from the queue");
}

#[test]
fn shelving_on_mission_control_still_belongs_to_s() {
    let mut t = scenario();
    t.keys([KeyCode::Char('s')]);
    assert_eq!(t.app.showcase.mode, None);
    let said = t.app.notice.clone().unwrap_or_default();
    assert!(
        said.contains("can't be shelved"),
        "s reached the inbox: {said:?}"
    );
}

#[test]
fn the_constellation_moves_and_the_spectrum_has_bars() {
    let mut t = calm();
    t.keys([KeyCode::Char('z')]);
    let first = render_to_string(&t.app, 160, 48);
    for _ in 0..5 {
        step(&mut t, 400);
    }
    let later = render_to_string(&t.app, 160, 48);
    assert_ne!(first, later, "the agents orbit");
    for name in ["demo-shop", "api", "docs"] {
        assert!(later.contains(name), "{name} is labelled");
    }
    assert!(later.contains("◉"), "the core");
    t.keys([KeyCode::Char('S')]);
    let bars = render_to_string(&t.app, 160, 48);
    assert!(bars.contains("▲ prompt") && bars.contains("▼ output"));
    assert!(bars.contains('█'));
}

#[test]
fn a_p1_or_p2_item_freezes_the_showcase_and_says_so() {
    let mut t = calm();
    t.keys([KeyCode::Char('z')]);
    for _ in 0..3 {
        step(&mut t, 300);
    }
    assert!(!t.app.showcase.frozen());
    let moving = t.app.showcase.clock_ms;
    assert!(moving > 0, "time passes while calm");

    // A risky call is held: a P1 in the inbox.
    let job = t.runner.request(0).job_id;
    t.inbox.push(Some(job), "Bash", "git push origin main");
    step(&mut t, 300);
    assert!(alerts(&t) > 0, "the item is in the inbox");
    assert!(t.app.showcase.frozen());
    let frozen_at = t.app.showcase.clock_ms;
    let a = render_to_string(&t.app, 160, 48);
    for _ in 0..8 {
        step(&mut t, 500);
        t.app.frame += 1;
    }
    let b = render_to_string(&t.app, 160, 48);
    assert_eq!(t.app.showcase.clock_ms, frozen_at, "the clock stands still");
    assert_eq!(a, b, "the picture does not change while frozen");
    let last = b.lines().last().unwrap_or_default();
    assert!(
        last.contains("P1") && last.contains("held still"),
        "one banner line: {last:?}"
    );
    assert_eq!(
        t.app.frame_time(),
        std::time::Duration::from_millis(250),
        "and the loop idles"
    );

    // Dimmed: the same star is darker than before the freeze.
    let tint = |app: &crate::app::App| -> Color {
        let buf = render_buffer(app, 160, 48);
        (0..48u16)
            .flat_map(|y| (0..160u16).map(move |x| (x, y)))
            .find(|&(x, y)| buf[(x, y)].symbol() == "◉")
            .map_or(Color::Reset, |(x, y)| buf[(x, y)].fg)
    };
    let held = tint(&t.app);

    // Answer it: the screens move again.
    t.keys([KeyCode::Char('n')]);
    for _ in 0..3 {
        step(&mut t, 300);
    }
    assert_eq!(alerts(&t), 0, "nothing needs the user now");
    assert!(!t.app.showcase.frozen());
    assert!(t.app.showcase.clock_ms > frozen_at, "time runs again");
    assert_ne!(tint(&t.app), held, "and the dimming is gone");
}

#[test]
fn a_held_call_makes_its_star_flare_while_everything_else_is_dim() {
    let mut t = scenario();
    t.keys([KeyCode::Char('z')]);
    assert!(t.app.showcase.frozen(), "the scenario has held calls");
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains("needs you"), "the star that waits is named");
    assert!(out.contains("✦"));
}

#[test]
fn reduced_and_off_motion_keep_the_picture_still() {
    for setting in [MotionSetting::Reduced, MotionSetting::Off] {
        let mut t = calm();
        t.app.look.motion = setting;
        t.keys([KeyCode::Char('z')]);
        let a = render_to_string(&t.app, 160, 48);
        for _ in 0..5 {
            step(&mut t, 400);
        }
        assert_eq!(a, render_to_string(&t.app, 160, 48), "{setting:?}");
        assert_eq!(t.app.showcase.clock_ms, 0);
        assert_eq!(t.app.look.fps(), 4);
    }
    let mut t = calm();
    t.app.look.motion = MotionSetting::Auto;
    t.app.look.auto_motion = crate::ui::motion::AutoMotion::from_vars(|k| k == "SSH_TTY");
    assert_eq!(t.app.look.tier(), Motion::Reduced);
    t.keys([KeyCode::Char('z')]);
    let footer = render_to_string(&t.app, 160, 48);
    assert!(footer.contains("motion reduced (ssh)"));
}

fn lines(s: &str) -> Vec<&str> {
    s.lines().collect()
}

#[test]
fn snapshot_constellation_at_160_by_48() {
    let mut t = calm();
    t.keys([KeyCode::Char('z')]);
    for _ in 0..4 {
        step(&mut t, 250);
    }
    let out = render_to_string(&t.app, 160, 48);
    let rows = lines(&out);
    assert_eq!(rows.len(), 48);
    assert!(rows[0].contains("SWARMUX") && rows[0].contains("constellation"));
    assert!(rows[0].contains("3 agents"));
    assert!(rows[47].contains("z/esc leave") && rows[47].contains("S spectrum"));
    assert!(out.contains('⠁') || out.chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c)));
    assert_eq!(out, render_to_string(&t.app, 160, 48), "deterministic");
}

#[test]
fn snapshot_spectrum_at_160_by_48() {
    let mut t = calm();
    t.keys([KeyCode::Char('S')]);
    for _ in 0..4 {
        step(&mut t, 250);
    }
    let out = render_to_string(&t.app, 160, 48);
    let rows = lines(&out);
    assert_eq!(rows.len(), 48);
    assert!(rows[0].contains("spectrum"));
    assert!(rows[47].contains("z constellation"));
    for name in ["demo-shop", "api", "docs"] {
        assert!(rows[46].contains(name), "{name} under its bars");
    }
    assert_eq!(out, render_to_string(&t.app, 160, 48), "deterministic");
}

/// Prints both screens for a human to look at:
/// `cargo test -p swarmux-tui show_the_showcase -- --ignored --nocapture`
#[test]
#[ignore = "prints, asserts nothing"]
fn show_the_showcase() {
    let mut t = calm();
    t.keys([KeyCode::Char('z')]);
    for _ in 0..4 {
        step(&mut t, 250);
    }
    println!("{}", render_to_string(&t.app, 160, 48));
    t.keys([KeyCode::Char('S')]);
    println!("{}", render_to_string(&t.app, 160, 48));
}

/// Idle CPU of the showcase at full motion, 160 x 48, with the real frame
/// pacing: the process's CPU time over the wall time. Rendering goes to a
/// memory buffer, so the terminal's own cost is not in the figure.
/// `cargo test --release -p swarmux-tui idle_cpu -- --ignored --nocapture`
#[test]
#[ignore = "measures for several seconds"]
fn idle_cpu_of_the_showcase_screens() {
    use std::time::{Duration, Instant};
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

    let pid = Pid::from_u32(std::process::id());
    let cpu_ms = |sys: &mut System| -> u64 {
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_cpu(),
        );
        sys.process(pid).map_or(0, |p| p.accumulated_cpu_time())
    };
    let mut sys = System::new().expect("a process table");
    for (label, key, seconds) in [
        ("constellation", 'z', 12u64),
        ("spectrum", 'S', 12),
        ("mission control", '1', 12),
    ] {
        let mut t = calm();
        if key != '1' {
            t.keys([KeyCode::Char(key)]);
        }
        let frame_time = t.app.frame_time();
        let before = cpu_ms(&mut sys);
        let start = Instant::now();
        let mut frames = 0u32;
        let (mut update_time, mut draw_time) = (Duration::ZERO, Duration::ZERO);
        while start.elapsed() < Duration::from_secs(seconds) {
            let tick = Instant::now();
            step(&mut t, frame_time.as_millis() as i64);
            let updated = tick.elapsed();
            let buf = render_buffer(&t.app, 160, 48);
            std::hint::black_box(&buf);
            update_time += updated;
            draw_time += tick.elapsed() - updated;
            t.app.frame += 1;
            frames += 1;
            if let Some(rest) = frame_time.checked_sub(tick.elapsed()) {
                std::thread::sleep(rest);
            }
        }
        let wall = start.elapsed().as_secs_f64();
        let used = (cpu_ms(&mut sys) - before) as f64 / 1000.0;
        let percent = used / wall * 100.0;
        println!(
            "{label}: {frames} frames in {wall:.1} s ({:.0} fps), cpu {used:.2} s = {percent:.2}% of one core",
            f64::from(frames) / wall,
        );
        if key != '1' {
            assert!(percent < 2.0, "{label} must idle under 2% CPU");
        }
        println!(
            "    per frame: update {:.2} ms, draw {:.2} ms",
            update_time.as_secs_f64() * 1000.0 / f64::from(frames),
            draw_time.as_secs_f64() * 1000.0 / f64::from(frames)
        );
    }
}
