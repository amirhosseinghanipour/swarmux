use crossterm::event::KeyCode;
use ratatui::style::Color;
use swarmux_application::ports::{Clock, RunnerMsg};
use swarmux_application::{
    Collected, Commit, DiffStat, GitStatus, GithubStatus, PullRequest, SystemStats,
};
use swarmux_domain::telemetry::{
    BurnRate, DaySpend, Environment, FlowBucket, HOUR, MINUTE, McpServer, NamedSpend, RateLimit,
    Spend, Todo, TodoStatus, UsageReport,
};
use swarmux_domain::{Actor, AgentEvent};

use super::layout;
use crate::app::Screen;
use crate::app::tests_support::{TestApp, test_app};
use crate::ui::look::IconSet;
use crate::ui::{render_buffer, render_to_string};

fn emit(t: &TestApp, i: usize, e: AgentEvent) {
    t.runner.emit(i, RunnerMsg::Event(e));
}

fn init(t: &TestApp, i: usize, model: &str) {
    let id = t.runner.request(i).session_id.to_string();
    emit(
        t,
        i,
        AgentEvent::Init {
            session_id: id,
            model: model.into(),
            cwd: "/work".into(),
            permission_mode: "acceptEdits".into(),
        },
    );
}

fn usage(t: &TestApp, i: usize, cache_read: u64) {
    emit(
        t,
        i,
        AgentEvent::Usage {
            input: 800,
            output: 300,
            cache_read,
            cache_creation: 900,
            actor: Actor::Lead,
        },
    );
}

fn finish(t: &TestApp, i: usize, is_error: bool, text: &str) {
    emit(
        t,
        i,
        AgentEvent::Result {
            is_error,
            subtype: if is_error {
                "error_max_turns"
            } else {
                "success"
            }
            .into(),
            cost_usd: 0.41,
            duration_ms: 42_000,
            num_turns: 9,
            tokens_in: 1_200,
            tokens_out: 5_400,
            cache_read: 182_400,
            context_window: Some(200_000),
            subagents_spawned: 0,
            text: text.into(),
        },
    );
}

fn todo(content: &str, status: TodoStatus) -> Todo {
    Todo {
        content: content.into(),
        status,
        active_form: None,
    }
}

fn limit(window: &str, used: f64, resets_in: i64, now: i64) -> AgentEvent {
    AgentEvent::RateLimit(RateLimit {
        window: window.into(),
        status: "allowed".into(),
        utilization: Some(used),
        resets_at: Some(now + resets_in),
        surpassed_threshold: None,
    })
}

/// Four sessions: one with a risky held call, one with an ordinary held
/// call, one finished, one failed; limits, spend and machine readings.
pub(crate) fn scenario() -> TestApp {
    let mut t = test_app();
    t.with_session("demo-shop");
    t.send("refactor the checkout flow to use the new payment client");
    init(&t, 0, "claude-opus-4-5");
    usage(&t, 0, 91_000);
    t.clock.advance(1_000);
    for (i, (name, summary)) in [
        ("Read", "src/checkout/cart.rs"),
        ("Edit", "src/checkout/cart.rs"),
        ("Bash", "cargo test checkout"),
    ]
    .into_iter()
    .enumerate()
    {
        emit(
            &t,
            0,
            AgentEvent::ToolUse {
                id: format!("t{i}"),
                name: name.into(),
                summary: summary.into(),
                actor: Actor::Lead,
            },
        );
    }
    emit(
        &t,
        0,
        AgentEvent::Todos {
            todos: vec![
                todo("map the cart flow", TodoStatus::Completed),
                todo("swap in the payment client", TodoStatus::InProgress),
                todo("update the tests", TodoStatus::Pending),
            ],
            actor: Actor::Lead,
        },
    );
    emit(
        &t,
        0,
        AgentEvent::Environment(Box::new(Environment {
            session_id: t.runner.request(0).session_id.to_string(),
            claude_code_version: Some("2.1.3".into()),
            model: "claude-opus-4-5".into(),
            permission_mode: "acceptEdits".into(),
            mcp_servers: vec![
                McpServer {
                    name: "github".into(),
                    status: "connected".into(),
                },
                McpServer {
                    name: "postgres".into(),
                    status: "failed".into(),
                },
            ],
            ..Environment::default()
        })),
    );
    t.clock.advance(1_000);
    t.with_session("api");
    t.send("fix the flaky retry test in the http layer");
    init(&t, 1, "claude-sonnet-4-5");
    usage(&t, 1, 40_000);
    t.clock.advance(1_000);
    t.with_session("docs");
    t.send("update the changelog for 0.4");
    init(&t, 2, "claude-haiku-4-5");
    t.app.update();
    t.clock.advance(5_000);
    finish(&t, 2, false, "Changelog updated with 14 entries.");
    t.app.update();
    t.with_session("infra");
    t.send("bump the base image and rebuild");
    init(&t, 3, "claude-sonnet-4-5");
    t.app.update();
    t.clock.advance(2_000);
    finish(&t, 3, true, "ran out of turns");
    let now = t.clock.now();
    emit(&t, 0, limit("five_hour", 0.30, 2 * HOUR + 14 * MINUTE, now));
    emit(&t, 0, limit("seven_day", 0.71, 3 * 24 * HOUR, now));
    t.app.update();
    t.clock.advance(10 * MINUTE);
    let now = t.clock.now();
    emit(&t, 0, limit("five_hour", 0.42, 2 * HOUR + 4 * MINUTE, now));
    usage(&t, 1, 44_000);
    let job = |n: usize| t.runner.request(n).job_id;
    t.inbox
        .push(Some(job(0)), "Bash", "git push origin feat/checkout");
    t.inbox.push(Some(job(1)), "Edit", "src/http/retry.rs");
    set_feed(&t, now);
    t.clock.advance(1_000);
    t.app.update();
    t.app.screen = Screen::Mission;
    t
}

#[allow(clippy::field_reassign_with_default)]
fn set_feed(t: &TestApp, now: i64) {
    let mut snap = swarmux_application::TelemetrySnapshot::default();
    snap.usage = Collected {
        enabled: true,
        data: Some(UsageReport {
            today: Spend {
                cost: 12.84,
                ..Spend::default()
            },
            burn: Some(BurnRate {
                usd_per_hour: 3.2,
                tokens_per_minute: 4_100.0,
            }),
            live_sessions: 3,
            cache_hit_ratio_today: Some(0.91),
            models_today: vec![
                named("claude-opus-4-5", 7.1, 0.55),
                named("claude-sonnet-4-5", 4.6, 0.36),
                named("claude-haiku-4-5", 1.1, 0.09),
            ],
            days: (0..90)
                .map(|i| DaySpend {
                    day: i,
                    date: format!("d{i}"),
                    weekday: (i % 7) as u32,
                    spend: Spend {
                        messages: ((i * 7) % 23) as u64,
                        ..Spend::default()
                    },
                    level: 0,
                })
                .collect(),
            flow: (0..60)
                .map(|m| FlowBucket {
                    minute: m,
                    input: 800 + (m as u64 * 37) % 500,
                    output: 300,
                    cache_read: 4_000 + (m as u64 * 91) % 3_000,
                    cache_write: 200,
                })
                .collect(),
            ..UsageReport::default()
        }),
        updated_at: Some(now),
        ..Collected::default()
    };
    snap.system = Collected {
        enabled: true,
        data: Some(SystemStats {
            cpu: 37.0,
            mem_used: 6 << 30,
            mem_total: 16 << 30,
            ..SystemStats::default()
        }),
        updated_at: Some(now),
        ..Collected::default()
    };
    snap.git = Collected {
        enabled: true,
        data: Some(GitStatus {
            branch: Some("feat/checkout".into()),
            ahead: 2,
            staged: 1,
            unstaged: 3,
            untracked: 1,
            diff: DiffStat {
                files: 4,
                insertions: 120,
                deletions: 31,
            },
            commits: vec![
                Commit {
                    hash: "a1b2c3d".into(),
                    subject: "feat: payment client".into(),
                    at: now,
                },
                Commit {
                    hash: "9f8e7d6".into(),
                    subject: "test: cart totals".into(),
                    at: now,
                },
            ],
            ..GitStatus::default()
        }),
        updated_at: Some(now),
        ..Collected::default()
    };
    snap.github = Collected {
        enabled: true,
        data: Some(GithubStatus {
            repo: Some("acme/shop".into()),
            pull_requests: vec![PullRequest {
                number: 42,
                title: "Checkout on the new payment client".into(),
                state: "OPEN".into(),
                review_decision: Some("REVIEW_REQUIRED".into()),
                draft: false,
                branch: "feat/checkout".into(),
            }],
            ci: None,
        }),
        updated_at: Some(now),
        ..Collected::default()
    };
    // The running sessions report their status lines.
    snap.statuslines = vec![swarmux_application::StatuslineView {
        session_id: "claude-0".into(),
        job: Some(t.runner.request(0).job_id),
        model: None,
        cwd: None,
        cost_usd: None,
        context_used: None,
        cache: None,
        pr: None,
        observed_at: now,
    }];
    t.feed.set(snap);
}

fn named(name: &str, cost: f64, share: f64) -> NamedSpend {
    NamedSpend {
        name: name.into(),
        spend: Spend {
            cost,
            ..Spend::default()
        },
        share,
        last_at: 0,
    }
}

fn tier(t: &mut TestApp, icons: IconSet) {
    t.app.look.icons = icons;
}

const SIZES: [(u16, u16); 5] = [(80, 24), (120, 36), (160, 48), (240, 64), (300, 80)];
const TIERS: [IconSet; 3] = [IconSet::Ascii, IconSet::Unicode, IconSet::Nerd];

fn position(out: &str, needle: &str) -> Option<(usize, usize)> {
    out.lines()
        .enumerate()
        .find_map(|(y, l)| l.find(needle).map(|x| (y, l[..x].chars().count())))
}

#[test]
fn snapshot_160x48() {
    let mut t = scenario();
    tier(&mut t, IconSet::Unicode);
    let out = render_to_string(&t.app, 160, 48);
    println!("{out}");
    for needle in [
        "needs you",
        "P1 1",
        "git push origin feat/checkout",
        "2:00 left",
        "y confirm (name the target) · n deny",
        "wall",
        "4 sessions",
        "demo-shop",
        "changes",
        "5h ",
        "7d ",
        "hits cap ~",
        "resets 2h",
        "burn $3.20/h",
        "cpu 37% mem 38%",
        "today $12.84",
        "fix the flaky retry test",
    ] {
        assert!(out.contains(needle), "missing {needle:?}\n{out}");
    }
    assert!(!out.contains("make the window bigger"));
}

#[test]
fn snapshot_80x24() {
    let mut t = scenario();
    tier(&mut t, IconSet::Unicode);
    let out = render_to_string(&t.app, 80, 24);
    println!("{out}");
    for needle in [
        "needs you",
        "P1 1",
        "wall",
        "5h ",
        "7d ",
        "demo-shop",
        "burn $3.20/h",
    ] {
        assert!(out.contains(needle), "missing {needle:?}\n{out}");
    }
    assert!(
        !out.contains("changes"),
        "the change log needs a taller window"
    );
}

#[test]
fn every_size_and_tier_renders_the_same_facts() {
    for (w, h) in SIZES {
        for icons in TIERS {
            let mut t = scenario();
            tier(&mut t, icons);
            let out = render_to_string(&t.app, w, h);
            for needle in ["needs you", "wall", "demo-shop", "git push", "5h "] {
                assert!(
                    out.contains(needle),
                    "{w}x{h} {icons:?} lacks {needle:?}\n{out}"
                );
            }
            let changes = out.contains("changes");
            let standard = layout::density(w, h) >= layout::Density::Standard;
            assert_eq!(changes, standard, "{w}x{h} {icons:?}\n{out}");
            assert!(out.contains("4 sessions"), "{w}x{h}");
        }
    }
}

#[test]
fn the_icon_tier_changes_the_marks_not_the_facts() {
    let mut t = scenario();
    tier(&mut t, IconSet::Ascii);
    let ascii = render_to_string(&t.app, 120, 40);
    tier(&mut t, IconSet::Unicode);
    let unicode = render_to_string(&t.app, 120, 40);
    tier(&mut t, IconSet::Nerd);
    let nerd = render_to_string(&t.app, 120, 40);
    assert!(unicode.contains('◆') && !ascii.contains('◆') && !nerd.contains('◆'));
    assert!(nerd.contains('\u{f132}') && !unicode.contains('\u{f132}'));
    assert!(ascii.contains(" ! approval"), "{ascii}");
    assert!(unicode.contains("◆ approval"), "{unicode}");
}

#[test]
fn inbox_is_ordered_by_priority_then_age() {
    let t = scenario();
    let out = render_to_string(&t.app, 160, 48);
    // The held call also shows in the overlay at the bottom: look above it.
    let top: String = out
        .lines()
        .take(30)
        .map(|l| l.chars().take(62).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    let at = |needle: &str| {
        position(&top, needle)
            .unwrap_or_else(|| panic!("{needle}\n{out}"))
            .0
    };
    let rows = [
        at("git push origin"),
        at("src/http/retry.rs"),
        at("bump the base image"),
        at("update the changelog"),
    ];
    assert!(rows.windows(2).all(|w| w[0] < w[1]), "{rows:?}\n{out}");
    assert!(out.contains("P1 1 · P2 2 · P3 1"), "{out}");
    let ids: Vec<_> = t
        .app
        .mission
        .items
        .iter()
        .map(|i| i.id.to_string())
        .collect();
    assert_eq!(ids.len(), 4);
    assert!(ids[0].starts_with("approval:") && ids[1].starts_with("approval:"));
    assert!(
        ids[2].ends_with(":failed") && ids[3].ends_with(":result"),
        "{ids:?}"
    );
}

#[test]
fn wall_tiles_do_not_move_when_a_state_changes() {
    let mut t = scenario();
    let before = render_to_string(&t.app, 160, 48);
    // The running sessions finish; the finished ones fail.
    finish(&t, 0, false, "all done");
    finish(&t, 1, true, "gave up");
    t.app.update();
    t.clock.advance(1_000);
    t.app.update();
    let after = render_to_string(&t.app, 160, 48);
    assert_ne!(before, after, "something did change");
    // A tile's title is its sigil and name; only the wall is right of column 60.
    let wall_places = |out: &str| {
        ["D demo-shop", "A api", "D docs", "I infra"].map(|n| {
            out.lines().enumerate().find_map(|(y, l)| {
                l.char_indices()
                    .find(|(x, _)| l[*x..].starts_with(n) && l[..*x].chars().count() > 60)
                    .map(|(x, _)| (y, l[..x].chars().count()))
            })
        })
    };
    assert_eq!(wall_places(&before), wall_places(&after));
    assert!(
        wall_places(&after).iter().all(Option::is_some),
        "{:?}",
        wall_places(&after)
    );
}

#[test]
fn stale_feeds_tell_their_age() {
    let mut t = scenario();
    let fresh = render_to_string(&t.app, 160, 48);
    assert!(!fresh.contains("old"), "{fresh}");
    t.clock.advance(90_000);
    t.app.update();
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains("usage 1m old"), "{out}");
    assert!(out.contains("system 1m old"), "{out}");
    // Narrow windows tell too.
    let narrow = render_to_string(&t.app, 80, 24);
    assert!(narrow.contains("usage 1m old"), "{narrow}");
}

#[test]
fn a_stale_limit_reading_is_marked() {
    let mut t = scenario();
    // The five-hour window has reset since Claude Code reported it.
    t.clock.advance(3 * HOUR);
    t.app.update();
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains("stale"), "{out}");
}

#[test]
fn empty_data_shows_calm_hints_never_blank_panels() {
    for (w, h) in SIZES {
        for icons in TIERS {
            let mut t = test_app();
            tier(&mut t, icons);
            let out = render_to_string(&t.app, w, h);
            assert!(out.contains("needs you"), "{w}x{h}\n{out}");
            assert!(out.contains("add a session"), "{w}x{h}\n{out}");
            assert!(out.contains("no reading yet"), "{w}x{h}\n{out}");
            assert!(out.contains("no sessions yet"), "{w}x{h}\n{out}");
            if layout::density(w, h) >= layout::Density::Standard {
                assert!(out.contains("nothing has changed yet"), "{out}");
            }
            // With a session but nothing to do.
            t.with_session("demo");
            t.app.screen = Screen::Mission;
            let out = render_to_string(&t.app, w, h);
            assert!(out.contains("all quiet"), "{w}x{h}\n{out}");
            assert!(out.contains("demo"), "{out}");
        }
    }
}

#[test]
fn too_small_windows_ask_for_more_room() {
    let t = scenario();
    let out = render_to_string(&t.app, 50, 10);
    assert!(out.contains("make the window bigger"), "{out}");
}

#[test]
fn y_answers_the_selected_approval_not_the_oldest() {
    let mut t = scenario();
    let ids: Vec<_> = t
        .app
        .mission
        .items
        .iter()
        .filter_map(|i| i.request)
        .collect();
    assert_eq!(ids.len(), 2);
    // The git push (first pushed) is P1 and selected: the overlay shows it.
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains("approval needed"), "{out}");
    t.keys([KeyCode::Down]);
    let selected = t.app.mission.selected_item().and_then(|i| i.request);
    assert_eq!(selected, Some(ids[1]));
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains("Edit src/http/retry.rs"), "{out}");
    t.keys([KeyCode::Char('y')]);
    assert!(t.inbox.answer_to(ids[1]).is_some());
    assert!(t.inbox.answer_to(ids[0]).is_none(), "the other stays held");
    t.app.update();
    assert_eq!(t.app.engine.guard.pending_count(), 1);
    assert_eq!(
        t.app
            .mission
            .items
            .iter()
            .filter(|i| i.request.is_some())
            .count(),
        1
    );
}

#[test]
fn approvals_still_answer_from_other_screens_oldest_first() {
    let mut t = scenario();
    t.app.screen = Screen::Queue;
    t.keys([KeyCode::Char('n')]);
    let first = t.app.mission.items.iter().find_map(|i| i.request);
    assert!(t.inbox.answer_to(first.unwrap()).is_some());
}

#[test]
fn x_dismisses_results_but_not_held_calls() {
    let mut t = scenario();
    t.keys([KeyCode::Char('x')]);
    assert_eq!(t.app.mission.items.len(), 4, "selected is a held call");
    assert!(t.app.notice.as_deref().unwrap_or("").contains("stay until"));
    t.keys([KeyCode::Down, KeyCode::Down, KeyCode::Down]);
    assert!(
        t.app
            .mission
            .selected_item()
            .unwrap()
            .id
            .as_str()
            .ends_with(":result")
    );
    t.keys([KeyCode::Char('x')]);
    assert_eq!(t.app.mission.items.len(), 3);
    t.app.update();
    assert_eq!(t.app.mission.items.len(), 3, "it stays dismissed");
}

#[test]
fn enter_opens_the_job_in_focus_and_reads_its_items() {
    let mut t = scenario();
    t.keys([KeyCode::Down, KeyCode::Down]);
    let item = t.app.mission.selected_item().unwrap().clone();
    assert!(item.id.as_str().ends_with(":failed"));
    t.keys([KeyCode::Enter]);
    assert_eq!(t.app.screen, Screen::Dashboard);
    assert_eq!(t.app.dashboard.followed, item.job);
    assert!(t.app.mission.items.iter().all(|i| i.id != item.id));
    t.keys([KeyCode::Char('1')]);
    assert_eq!(t.app.screen, Screen::Mission);
}

#[test]
fn a_p1_border_breathes_but_not_in_low_power() {
    let mut t = scenario();
    let corner = |t: &TestApp| -> Color {
        let buf = render_buffer(&t.app, 160, 48);
        let (x, y) = (0..48u16)
            .find_map(|y| (buf[(0, y)].symbol() == "╭" && y > 4).then_some((0, y)))
            .expect("the inbox corner");
        buf[(x, y)].fg
    };
    t.app.frame = 0;
    let bright = corner(&t);
    t.app.frame = 10;
    let dim = corner(&t);
    assert_ne!(bright, dim, "0.5 Hz at 10 fps: dimmest after one second");
    t.app.frame = 20;
    assert_eq!(corner(&t), bright, "one breath every two seconds");
    t.app.look.low_power = true;
    t.app.frame = 0;
    let a = corner(&t);
    t.app.frame = 2;
    assert_eq!(corner(&t), a, "no motion in low power");

    // Without a P1 item the border stays the accent.
    let mut quiet = test_app();
    quiet.with_session("demo");
    quiet.app.screen = Screen::Mission;
    quiet.app.frame = 0;
    let a = corner(&quiet);
    quiet.app.frame = 10;
    assert_eq!(corner(&quiet), a);
}

#[test]
fn many_sessions_become_a_list_with_a_count() {
    let mut t = test_app();
    for i in 0..10 {
        t.with_session(&format!("session-{i}"));
        t.clock.advance(10);
    }
    t.app.screen = Screen::Mission;
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains("10 sessions"), "{out}");
    assert!(out.contains("session-9"), "{out}");
    assert!(
        !out.contains("› "),
        "compact rows carry no task line\n{out}"
    );
}

/// Facts every tier shows, whatever its size.
fn shows_the_basics(out: &str, what: &str) {
    for needle in [
        "needs you",
        "P1 1",
        "git push origin",
        "wall",
        "demo-shop",
        "5h ",
        "7d ",
        "hits cap ~",
        "burn $3.20/h",
    ] {
        assert!(out.contains(needle), "{what} lacks {needle:?}\n{out}");
    }
}

#[test]
fn snapshot_120x36_is_compact() {
    let mut t = scenario();
    tier(&mut t, IconSet::Unicode);
    let out = render_to_string(&t.app, 120, 36);
    println!("{out}");
    shows_the_basics(&out, "120x36");
    for absent in ["changes", "token flow", "model mix", "12 weeks", "peek"] {
        assert!(!out.contains(absent), "compact has no {absent:?}\n{out}");
    }
    assert!(!out.contains("slot "), "the list has no card slots");
}

#[test]
fn snapshot_160x48_is_standard() {
    let mut t = scenario();
    tier(&mut t, IconSet::Unicode);
    let out = render_to_string(&t.app, 160, 48);
    shows_the_basics(&out, "160x48");
    for present in ["changes", "token flow", "ctx ", "› refactor the checkout"] {
        assert!(out.contains(present), "standard shows {present:?}\n{out}");
    }
    for absent in ["model mix", "12 weeks", "peek", "slot "] {
        assert!(!out.contains(absent), "standard has no {absent:?}\n{out}");
    }
}

#[test]
fn snapshot_240x64_is_full() {
    let mut t = scenario();
    tier(&mut t, IconSet::Unicode);
    let out = render_to_string(&t.app, 240, 64);
    println!("{out}");
    shows_the_basics(&out, "240x64");
    for present in [
        "burn ",
        "model mix",
        "swarmux 0.1.0 · claude 2.1.3",
        "agents",
        "token flow",
        "tools",
        "files",
        "git",
        "pull requests",
        "#42",
        "mcp",
        "postgres",
        "system",
        "12 weeks",
        "changes",
        "todo ",
        "1/3",
        "held call",
        "slot 5 · free",
        "src/checkout/cart.rs",
        "feat/checkout",
    ] {
        assert!(out.contains(present), "full shows {present:?}\n{out}");
    }
    assert!(!out.contains("peek"), "the peek is ultrawide only");
}

#[test]
fn snapshot_300x80_is_ultrawide_with_a_peek() {
    let mut t = scenario();
    tier(&mut t, IconSet::Unicode);
    let out = render_to_string(&t.app, 300, 80);
    shows_the_basics(&out, "300x80");
    for present in [
        "peek",
        "model mix",
        "12 weeks",
        "todos",
        "recent",
        "swap in the payment client",
    ] {
        assert!(out.contains(present), "ultrawide shows {present:?}\n{out}");
    }
    // The peek follows the selection: the second approval is another job.
    t.keys([KeyCode::Down]);
    let out = render_to_string(&t.app, 300, 80);
    assert!(out.contains("#2 "), "{out}");
}

#[test]
fn only_the_top_inbox_item_animates() {
    // Two frames a second apart differ only inside the inbox column: its border and its
    // top item; every other cell is static between real updates.
    let mut t = scenario();
    t.app.frame = 0;
    let a = render_buffer(&t.app, 240, 64);
    t.app.frame = 10;
    let b = render_buffer(&t.app, 240, 64);
    let inbox_right = 60;
    for y in 0..64u16 {
        for x in 0..240u16 {
            let (p, q) = (&a[(x, y)], &b[(x, y)]);
            if p != q {
                assert!(
                    x < inbox_right,
                    "cell ({x},{y}) changed: {:?} -> {:?}",
                    p.symbol(),
                    q.symbol()
                );
            }
        }
    }
}

#[test]
fn every_size_renders_without_panicking() {
    let t = scenario();
    for w in (1..=320).step_by(13) {
        for h in (1..=90).step_by(7) {
            let _ = render_to_string(&t.app, w, h);
        }
    }
    let mut empty = test_app();
    empty.app.screen = Screen::Mission;
    for (w, h) in [
        (1, 1),
        (60, 12),
        (79, 23),
        (100, 30),
        (219, 59),
        (299, 79),
        (400, 120),
    ] {
        let _ = render_to_string(&empty.app, w, h);
        let _ = render_to_string(&t.app, w, h);
    }
}

/// Cost of one full-tier frame: `cargo test --release -p swarmux-tui
/// mission_render_cost -- --ignored --nocapture`.
#[test]
#[ignore = "timing, run in release"]
fn mission_render_cost() {
    use ratatui::{Terminal, backend::TestBackend};
    let t = scenario();
    let mut terminal = Terminal::new(TestBackend::new(200, 60)).expect("terminal");
    for _ in 0..20 {
        terminal.draw(|f| crate::ui::draw(f, &t.app)).expect("draw");
    }
    let mut times = Vec::new();
    for _ in 0..300 {
        let start = std::time::Instant::now();
        terminal.draw(|f| crate::ui::draw(f, &t.app)).expect("draw");
        times.push(start.elapsed());
    }
    times.sort();
    println!(
        "mission 200x60: median {:?}, p95 {:?}, max {:?}",
        times[times.len() / 2],
        times[times.len() * 95 / 100],
        times[times.len() - 1]
    );
}

#[test]
fn p_on_mission_prompts_the_session_of_the_selected_item() {
    let mut t = test_app();
    t.with_session("alpha");
    t.send("one");
    init(&t, 0, "sonnet");
    finish(&t, 0, false, "all done");
    t.with_session("beta");
    t.app.update();
    t.keys([KeyCode::Char('1')]);
    let alpha = t
        .app
        .sessions
        .iter()
        .find(|s| s.name == "alpha")
        .unwrap()
        .id;
    let beta = t.app.sessions.iter().find(|s| s.name == "beta").unwrap().id;
    assert_eq!(t.app.selected_session().map(|s| s.id), Some(beta));
    assert_eq!(
        t.app.mission.selected_item().and_then(|i| i.session),
        Some(alpha)
    );

    t.keys([KeyCode::Char('p')]);
    let out = render_to_string(&t.app, 120, 30);
    assert!(out.contains("prompt → alpha"), "{out}");
    t.keys("two".chars().map(KeyCode::Char));
    t.keys([KeyCode::Enter]);
    assert_eq!(t.app.screen, Screen::Mission, "stays on mission control");
    assert_eq!(t.runner.request(1).session_id, alpha);
    assert_eq!(t.runner.request(1).prompt, "two");
}

#[test]
fn capital_p_on_mission_asks_swarmux_to_route() {
    let mut t = test_app();
    t.with_session("alpha");
    t.app.update();
    t.keys([KeyCode::Char('1'), KeyCode::Char('P')]);
    let out = render_to_string(&t.app, 120, 30);
    assert!(out.contains("swarmux picks the session"), "{out}");
}

#[test]
fn p_on_mission_without_sessions_says_what_to_do() {
    let mut t = test_app();
    t.keys([KeyCode::Char('1'), KeyCode::Char('p')]);
    assert!(
        t.app
            .error
            .as_deref()
            .unwrap_or("")
            .contains("create a session")
    );
}

#[test]
fn the_approval_is_one_line_at_80x24_and_the_wall_stays() {
    let mut t = test_app();
    t.with_session("demo");
    t.send("hi");
    let job = t.app.current_job().unwrap().job_id;
    t.inbox.push(Some(job), "Bash", "make deploy");
    t.app.update();
    t.keys([KeyCode::Char('1')]);

    let small = render_to_string(&t.app, 80, 24);
    assert!(!small.contains("approval needed"), "{small}");
    assert!(small.contains("Bash make deploy"), "{small}");
    assert!(small.contains("y n a"), "{small}");
    assert!(small.contains("1 session"), "the wall header\n{small}");
    assert!(small.contains("demo"), "the tile\n{small}");
    let bar = small.lines().filter(|l| l.contains("y n a")).count();
    assert_eq!(bar, 1, "one line\n{small}");

    // Roomier windows keep the full overlay.
    let big = render_to_string(&t.app, 120, 40);
    assert!(big.contains("approval needed"), "{big}");
}

fn gate_record(
    job: swarmux_domain::JobId,
    outcome: swarmux_domain::gate::GateOutcome,
    blocks: u32,
) -> swarmux_application::GateRecord {
    use swarmux_domain::gate::{CheckRun, GateFindings, GateOrigin};
    swarmux_application::GateRecord {
        job: Some(job),
        session: "claude-1".into(),
        event: Some(swarmux_domain::HookEvent::Stop),
        root: "/work".into(),
        origin: GateOrigin::File,
        outcome,
        blocks,
        max_blocks: 3,
        findings: GateFindings {
            runs: vec![CheckRun {
                name: "cargo test".into(),
                command: "cargo test".into(),
                exit: Some(101),
                timed_out: false,
                spawn_error: None,
                duration_ms: 4_000,
                output: "assertion failed".into(),
                flaky: false,
            }],
            pins: Vec::new(),
            gaps: vec!["you said \"done\", but no test run succeeded in this session".into()],
            ..GateFindings::default()
        },
        digest: "swarmux gate: still failing after 3 blocks.\n✗ cargo test: `cargo test` exited 101 after 4.0 s".into(),
        problem: None,
    }
}

#[test]
fn gate_verdicts_become_wall_chips_changes_and_an_inbox_item() {
    use std::rc::Rc;
    use swarmux_application::GateService;
    use swarmux_application::ledger::LedgerService;
    use swarmux_application::testing::InMemoryLedger;
    use swarmux_domain::gate::GateOutcome;
    use swarmux_domain::{AttentionKind, ledger::LedgerKind};

    let mut t = test_app();
    t.with_session("oracle");
    t.send("make the answer right");
    init(&t, 0, "claude-sonnet-4-5");
    let (tx, rx) = std::sync::mpsc::channel();
    let ledger = Rc::new(InMemoryLedger::default());
    let service = Rc::new(LedgerService::new(ledger.clone(), t.clock.clone()));
    t.app.set_gate(GateService::new(
        rx,
        service,
        t.store.clone(),
        t.clock.clone(),
    ));
    t.app.screen = Screen::Mission;
    t.app.update();
    let out = render_to_string(&t.app, 240, 64);
    assert!(out.contains("gate runs when the agent stops"), "{out}");

    let job = t.app.mission.facts[0].job_id;
    tx.send(gate_record(job, GateOutcome::Block, 0)).unwrap();
    t.app.update();
    let out = render_to_string(&t.app, 240, 64);
    for needle in ["gate 1/3", "cargo test", "gate blocked a stop (1 of 3)"] {
        assert!(out.contains(needle), "missing {needle:?}\n{out}");
    }
    assert!(!out.contains("gate runs when the agent stops"));
    assert!(
        t.app
            .mission
            .items
            .iter()
            .all(|i| i.kind != AttentionKind::GateEscalated)
    );

    tx.send(gate_record(job, GateOutcome::Escalate, 3)).unwrap();
    t.app.update();
    let out = render_to_string(&t.app, 240, 64);
    assert!(out.contains("gate gave up"), "{out}");
    let item = t
        .app
        .mission
        .items
        .iter()
        .find(|i| i.kind == AttentionKind::GateEscalated)
        .expect("an inbox item");
    assert_eq!(item.title, "make the answer right");
    assert!(item.detail.starts_with("✗ cargo test"));
    let verdicts = ledger
        .entries()
        .iter()
        .filter(|e| e.draft.kind == LedgerKind::GateVerdict)
        .count();
    assert_eq!(verdicts, 2);
}

#[test]
fn watchdog_steps_show_in_the_change_log_focus_and_the_inbox() {
    use std::rc::Rc;
    use swarmux_application::ledger::LedgerService;
    use swarmux_application::testing::InMemoryLedger;
    use swarmux_application::{HookMessenger, NudgeBoard, Watchdog, WatchdogSettings};
    use swarmux_domain::ledger::LedgerKind;
    use swarmux_domain::{AttentionKind, LadderPolicy, Rung};

    let mut t = test_app();
    t.with_session("api");
    t.send("fix the build");
    init(&t, 0, "claude-sonnet-4-5");
    let ledger = Rc::new(InMemoryLedger::default());
    let settings = WatchdogSettings {
        ladder: LadderPolicy {
            rungs: vec![Rung::Nudge],
            ..LadderPolicy::default()
        },
        ..WatchdogSettings::default()
    };
    t.app.set_watchdog(
        Watchdog::new(
            settings,
            Rc::new(HookMessenger(NudgeBoard::new())),
            t.clock.clone(),
        )
        .with_ledger(Rc::new(LedgerService::new(ledger.clone(), t.clock.clone()))),
    );
    // Rereads the same two files over and over: nothing new.
    let read = |t: &TestApp, i: u32| {
        let id = format!("r{i}");
        emit(
            t,
            0,
            AgentEvent::ToolUse {
                id: id.clone(),
                name: "Read".into(),
                summary: if i.is_multiple_of(2) {
                    "src/a.rs"
                } else {
                    "src/b.rs"
                }
                .into(),
                actor: Actor::Lead,
            },
        );
        emit(
            t,
            0,
            AgentEvent::ToolResult {
                tool_use_id: id,
                is_error: false,
                preview: "fn a() {}".into(),
                actor: Actor::Lead,
            },
        );
    };
    for i in 0..17u32 {
        read(&t, i);
    }
    t.app.screen = Screen::Mission;
    t.app.update();
    let out = render_to_string(&t.app, 240, 64);
    assert!(out.contains("watchdog: nudge"), "{out}");
    let job = t.app.mission.facts[0].job_id;
    let progress = t.app.engine.jobs.progress(job).unwrap();
    assert_eq!(
        progress.recovery,
        ["nudge: waiting for the agent's next call"]
    );
    // Still going round once the nudge has had its time: only the inbox is left.
    t.clock.advance(2 * 60_000);
    read(&t, 17);
    t.app.update();
    let item = t
        .app
        .mission
        .items
        .iter()
        .find(|i| i.kind == AttentionKind::Recovery)
        .expect("an inbox item");
    assert_eq!(item.title, "fix the build");
    assert!(item.detail.contains("tried nudge"), "{}", item.detail);
    let rungs = ledger
        .entries()
        .iter()
        .filter(|e| {
            e.draft.kind == LedgerKind::Watchdog && e.draft.payload_json.contains("\"rung\"")
        })
        .count();
    assert!(rungs >= 2, "every rung is in the ledger");
}
