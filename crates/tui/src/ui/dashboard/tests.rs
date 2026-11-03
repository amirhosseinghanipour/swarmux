use crossterm::event::KeyCode;
use swarmux_application::ports::RunnerMsg;
use swarmux_domain::{Actor, AgentEvent};

use crate::app::tests_support::{TestApp, test_app};
use crate::ui::render_to_string;

fn ev(t: &TestApp, i: usize, e: AgentEvent) {
    t.runner.emit(i, RunnerMsg::Event(e));
}

fn sub(id: &str) -> Actor {
    Actor::Subagent(id.into())
}

fn usage(input: u64, output: u64, cache_read: u64, actor: Actor) -> AgentEvent {
    AgentEvent::Usage {
        input,
        output,
        cache_read,
        cache_creation: 900,
        actor,
    }
}

fn tool(id: &str, name: &str, summary: &str, actor: Actor) -> AgentEvent {
    AgentEvent::ToolUse {
        id: id.into(),
        name: name.into(),
        summary: summary.into(),
        actor,
    }
}

fn result(id: &str, text: &str, is_error: bool, actor: Actor) -> AgentEvent {
    AgentEvent::ToolResult {
        tool_use_id: id.into(),
        is_error,
        preview: text.into(),
        actor,
    }
}

fn done(text: &str, cost: f64) -> AgentEvent {
    AgentEvent::Result {
        is_error: false,
        subtype: "success".into(),
        cost_usd: cost,
        duration_ms: 42_000,
        num_turns: 9,
        tokens_in: 1_200,
        tokens_out: 5_400,
        cache_read: 182_400,
        context_window: Some(1_000_000),
        subagents_spawned: 3,
        text: text.into(),
    }
}

/// A session, review on, and a job with three subagents part-way through
/// (two still running). Returns the app.
pub(crate) fn running_job() -> TestApp {
    let mut t = test_app();
    t.catalog.define("worker", "sonnet");
    t.catalog.define("tester", "sonnet");
    t.catalog.define("scout", "haiku");
    t.with_session("demo-shop");
    t.keys([KeyCode::Char('v')]);
    t.send("add a /health endpoint with tests");
    let job = t.app.current_job().unwrap().job_id;
    let id = t.runner.request(0).session_id.to_string();
    ev(
        &t,
        0,
        AgentEvent::Init {
            session_id: id,
            model: "claude-sonnet-4-5".into(),
            cwd: "/work".into(),
            permission_mode: "default".into(),
        },
    );
    t.app.update();
    t.clock.advance(1_500);
    ev(
        &t,
        0,
        AgentEvent::Text {
            text: "Plan: map the routes, write the handler, add tests.".into(),
            actor: Actor::Lead,
        },
    );
    ev(&t, 0, usage(6, 410, 15_100, Actor::Lead));
    t.app.update();
    for (tool_name, subject) in [
        ("Read", "/w/src/app.py"),
        ("Grep", "route"),
        ("Glob", "tests/**"),
    ] {
        t.inbox.push(Some(job), tool_name, subject);
    }
    t.app.update();
    t.clock.advance(2_000);
    t.app.update();
    ev(
        &t,
        0,
        tool(
            "A",
            "Task",
            "worker: write the /health handler",
            Actor::Lead,
        ),
    );
    ev(
        &t,
        0,
        tool("B", "Task", "tester: write and run the tests", Actor::Lead),
    );
    ev(
        &t,
        0,
        tool("C", "Task", "scout: list the existing routes", Actor::Lead),
    );
    t.app.update();
    t.clock.advance(1_500);
    ev(&t, 0, tool("c1", "Grep", "@app.route", sub("C")));
    ev(&t, 0, usage(3, 900, 4_200, sub("C")));
    ev(
        &t,
        0,
        result("c1", "src/app.py:12 @app.route(\"/\")", false, sub("C")),
    );
    t.app.update();
    t.clock.advance(2_000);
    ev(&t, 0, usage(3, 1_400, 8_800, sub("C")));
    ev(
        &t,
        0,
        result("C", "2 routes: / and /items", false, Actor::Lead),
    );
    ev(&t, 0, tool("a1", "Edit", "/work/src/app.py", sub("A")));
    ev(&t, 0, usage(4, 1_800, 11_000, sub("A")));
    ev(&t, 0, result("a1", "ok", false, sub("A")));
    ev(&t, 0, tool("b1", "Bash", "pytest -q", sub("B")));
    ev(&t, 0, usage(5, 600, 9_300, sub("B")));
    ev(
        &t,
        0,
        result("b1", "22 passed, 6 failed in 3.1s", false, sub("B")),
    );
    t.inbox.push(Some(job), "Edit", "/w/src/app.py");
    t.inbox.push(Some(job), "Bash", "pytest -q");
    t.app.update();
    t.clock.advance(1_000);
    t.app.update();
    t
}

/// The same job finished, reviewed and passed.
pub(crate) fn reviewed_job() -> TestApp {
    let mut t = running_job();
    // Answer the approvals the unmatched calls are waiting on.
    t.keys([KeyCode::Char('y'), KeyCode::Char('y')]);
    t.app.update();
    t.clock.advance(4_000);
    ev(&t, 0, result("b2", "", false, sub("B")));
    ev(&t, 0, usage(5, 700, 12_100, sub("B")));
    ev(&t, 0, result("A", "handler added", false, Actor::Lead));
    ev(&t, 0, result("B", "28 passed", false, Actor::Lead));
    t.app.update();
    t.clock.advance(3_000);
    ev(&t, 0, usage(8, 1_100, 31_000, Actor::Lead));
    ev(
        &t,
        0,
        AgentEvent::Text {
            text: "Added GET /health returning {\"ok\": true}; 28 tests pass.".into(),
            actor: Actor::Lead,
        },
    );
    ev(&t, 0, done("Added GET /health; 28 tests pass.", 0.4123));
    t.app.update();
    t.app.update();
    t.clock.advance(2_000);
    ev(
        &t,
        1,
        AgentEvent::Init {
            session_id: "review".into(),
            model: "claude-opus-5-5".into(),
            cwd: "/work".into(),
            permission_mode: "default".into(),
        },
    );
    ev(&t, 1, tool("r1", "Read", "/work/src/app.py", Actor::Lead));
    ev(&t, 1, usage(4, 300, 22_000, Actor::Lead));
    ev(&t, 1, result("r1", "def health(): ...", false, Actor::Lead));
    t.app.update();
    t.clock.advance(3_000);
    ev(&t, 1, usage(4, 120, 23_100, Actor::Lead));
    ev(
        &t,
        1,
        AgentEvent::Result {
            is_error: false,
            subtype: "success".into(),
            cost_usd: 0.0311,
            duration_ms: 5_000,
            num_turns: 2,
            tokens_in: 8,
            tokens_out: 420,
            cache_read: 45_100,
            context_window: Some(1_000_000),
            subagents_spawned: 0,
            text: r#"{"verdict":"pass","concerns":[],"diffs_checked":3}"#.into(),
        },
    );
    t.app.update();
    t
}

#[test]
fn full_dashboard_shows_every_panel_with_real_numbers() {
    let t = reviewed_job();
    let out = render_to_string(&t.app, 160, 48);
    if let Ok(path) = std::env::var("SWARMUX_SNAPSHOT") {
        std::fs::write(path, &out).unwrap();
    }
    for needle in [
        "SWARMUX · SONNET 4.5 ORCHESTRATES · OPUS 5.5 AUDITS",
        "sonnet lead",
        "sonnet builder",
        "haiku",
        "> claude -p --model sonnet --session-id",
        "3 subagents mounted · reviewer done",
        "> ship_",
        "prompts go to",
        "demo-shop",
        "REVIEWER · Opus 5.5 · adversary",
        "<before a plan>",
        "<error again>",
        "<before done>",
        "» 0 concerns",
        "3 diffs approved [OK]",
        "Sonnet 4.5 · main session",
        "ctx 31.9k / 1.0M",
        "saved 182.4k tokens",
        "GUARD · fork layer · 7 decisions",
        "tool gate",
        "sharp: runs in code, the lead never sees it",
        ": delegate · 3 subagents",
        "worker",
        "tester",
        "scout",
        "Sonnet · ",
        "Haiku · ",
        "[context discarded · lean]",
        "back to Sonnet 4.5 · main session",
        "[✓] worker",
        "[✓] reviewer",
        "3 diffs verified · no edits until go",
        "session timeline",
        "tail -f session.log",
        "who sees what",
        "| step [ship]",
        "subagents [0/3 active]",
        "reviewer [passed]",
    ] {
        assert!(out.contains(needle), "missing {needle:?}\n{out}");
    }
    let steps = out.lines().find(|l| l.contains("steps")).unwrap();
    for phase in [
        "plan", "fork", "delegate", "parallel", "merge", "audit", "ship",
    ] {
        assert!(steps.contains(phase), "{phase} not on the axis: {steps}");
    }
}

#[test]
fn running_job_shows_parallel_work_and_spinners() {
    let mut t = running_job();
    t.app.frame = 2;
    let out = render_to_string(&t.app, 160, 48);
    for needle in [
        "> parallel_",
        "[/]",
        "[Bash: 22 passed, 6 failed in 3.1s]",
        "subagents [2/3 active]",
        "approval needed",
        "reviewer [on call]",
        "waits for the job to finish",
    ] {
        assert!(out.contains(needle), "missing {needle:?}\n{out}");
    }
}

#[test]
fn collapsed_layouts_keep_the_essentials() {
    let t = reviewed_job();
    let out = render_to_string(&t.app, 100, 30);
    assert!(!out.contains("who sees what"), "{out}");
    assert!(
        out.contains("REVIEWER · Opus 5.5 · » 0 concerns · 3 diffs approved [OK]"),
        "{out}"
    );
    assert!(out.contains(" subagents "), "{out}");
    assert!(out.contains("GUARD · fork layer"), "{out}");
    assert!(out.contains("tail -f session.log"), "{out}");

    let out = render_to_string(&t.app, 80, 24);
    for needle in [
        "> ship_",
        "REVIEWER",
        "main",
        "GUARD",
        "subagents",
        "back to",
        "tail -f",
    ] {
        assert!(out.contains(needle), "missing {needle:?}\n{out}");
    }
    assert!(!out.contains("session timeline"), "{out}");

    let out = render_to_string(&t.app, 130, 40);
    assert!(
        out.contains("session timeline") && !out.contains("who sees what"),
        "{out}"
    );

    let out = render_to_string(&t.app, 79, 30);
    assert!(out.contains("make the window bigger"), "{out}");
}

#[test]
fn every_size_renders_without_panicking() {
    let t = reviewed_job();
    let empty = test_app();
    for w in 20..=250u16 {
        for h in [3u16, 24, 31, 48] {
            render_to_string(&t.app, w, h);
        }
        render_to_string(&empty.app, w, 40);
    }
    for h in 1..=60u16 {
        render_to_string(&t.app, 120, h);
    }
}

#[test]
fn empty_dashboard_has_placeholders_not_numbers() {
    let mut t = test_app();
    t.app.screen = crate::app::Screen::Dashboard;
    let out = render_to_string(&t.app, 160, 48);
    assert!(out.contains("press 6 then n"), "{out}");
    assert!(out.contains("REVIEW OFF (v)"), "{out}");
    t.with_session("demo");
    let out = render_to_string(&t.app, 160, 48);
    assert!(
        out.contains("no job yet · press p to send a prompt to demo"),
        "{out}"
    );
    assert!(
        out.contains("subagents appear here when the lead delegates"),
        "{out}"
    );
    assert!(out.contains("effort [—]"), "{out}");
}

#[test]
fn v_toggles_review_for_the_next_jobs() {
    let mut t = test_app();
    t.with_session("demo");
    t.keys([KeyCode::Char('v')]);
    assert!(t.app.notice.as_deref().unwrap().starts_with("review on"));
    assert!(render_to_string(&t.app, 160, 48).contains("OPUS AUDITS"));
    t.send("hi");
    assert!(
        t.store
            .job_of(t.app.current_job().unwrap().job_id)
            .wants_review
    );
    t.keys([KeyCode::Char('v')]);
    assert_eq!(
        t.app.notice.as_deref(),
        Some("review off for the next jobs")
    );
}
