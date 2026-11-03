use std::path::Path;

use swarmux_domain::telemetry::{
    Environment, ExternalKind, ExternalSession, ExternalState, HOUR, InstructionFile, LimitSource,
    MINUTE, McpServer, PromptCache, PullRef, RateLimit, SessionUsage, Spend, Statusline,
    UsageReport, WindowReading,
};
use swarmux_domain::{Actor, AgentEvent, Session, SessionId};

use super::*;
use crate::ports::{RunnerMsg, TelemetryStore};
use crate::testing::{
    FakeRunner, FakeStatuslineFeed, FakeTelemetry, FakeTelemetryStore, FixedClock, InMemoryStore,
};

const NOW: i64 = 1_793_613_600_000;

struct Fixture {
    telemetry: TelemetryService,
    feed: Rc<FakeTelemetry>,
    jobs: JobService,
    runner: Rc<FakeRunner>,
    store: Rc<FakeTelemetryStore>,
    clock: Rc<FixedClock>,
    session: Session,
    sessions: Rc<InMemoryStore>,
}

fn fixture() -> Fixture {
    let sessions = Rc::new(InMemoryStore::default());
    let session = Session::new(SessionId::random(), "demo", "/w".into(), "sonnet", 0).unwrap();
    SessionRepository::insert(sessions.as_ref(), &session).unwrap();
    let clock = Rc::new(FixedClock::new(NOW));
    let runner = Rc::new(FakeRunner::default());
    let store = Rc::new(FakeTelemetryStore::default());
    let jobs = JobService::new(
        sessions.clone(),
        sessions.clone(),
        runner.clone(),
        clock.clone(),
    )
    .with_telemetry_store(store.clone());
    let feed = Rc::new(FakeTelemetry::default());
    let telemetry = TelemetryService::new(feed.clone(), sessions.clone(), clock.clone());
    Fixture {
        telemetry,
        feed,
        jobs,
        runner,
        store,
        clock,
        session,
        sessions,
    }
}

fn limit(window: &str, used: f64, resets_at: i64) -> RateLimit {
    RateLimit {
        window: window.into(),
        status: "allowed".into(),
        utilization: Some(used),
        resets_at: Some(resets_at),
        surpassed_threshold: None,
    }
}

fn wall_entry(id: &str) -> SessionUsage {
    SessionUsage {
        session_id: id.into(),
        project: "/w".into(),
        model: "claude-opus-5-5".into(),
        first_at: 0,
        last_at: 0,
        spend: Spend::default(),
        context: Default::default(),
        live: false,
        swarmux: false,
    }
}

impl Fixture {
    fn emit(&mut self, ev: AgentEvent) {
        self.runner.emit(0, RunnerMsg::Event(ev));
        self.jobs.pump();
    }
}

#[test]
fn snapshot_passes_the_feed_through_and_marks_swarmux_sessions() {
    let f = fixture();
    let mut snap = TelemetrySnapshot {
        offset_min: 60,
        ..TelemetrySnapshot::default()
    };
    snap.usage = Collected::pending();
    snap.usage.data = Some(UsageReport {
        sessions: vec![
            wall_entry(&f.session.id.to_string()),
            wall_entry("someone-else"),
        ],
        ..UsageReport::default()
    });
    f.feed.set(snap);
    let out = f.telemetry.snapshot(&f.jobs);
    assert_eq!(out.taken_at, NOW);
    assert_eq!(out.offset_min, 60);
    let wall = &out.usage.data.as_ref().unwrap().sessions;
    assert!(wall[0].swarmux);
    assert!(!wall[1].swarmux);
    f.telemetry.focus(Path::new("/w"));
    assert_eq!(f.feed.focused(), [Path::new("/w").to_path_buf()]);
}

#[test]
fn rate_limits_are_kept_projected_and_stored() {
    let mut f = fixture();
    f.jobs.dispatch(f.session.id, "go").unwrap();
    // Window opened 2 h ago, half used: at this pace it is full in 2 h.
    let reset = NOW + 3 * HOUR;
    f.emit(AgentEvent::RateLimit(limit("five_hour", 0.5, reset)));
    f.emit(AgentEvent::RateLimit(limit(
        "seven_day",
        0.1,
        NOW + 100 * HOUR,
    )));
    let snap = f.telemetry.snapshot(&f.jobs);
    let windows: Vec<&str> = snap.limits.iter().map(|l| l.window.as_str()).collect();
    assert_eq!(windows, ["five_hour", "seven_day"]);
    let five = &snap.limits[0];
    assert_eq!(five.resets_in, Some(3 * HOUR));
    assert_eq!(five.projection.map(|p| p.hits_at), Some(NOW + 2 * HOUR));
    // 2026-11-02 12:00 UTC is 13:00 at +01:00.
    f.feed.set(TelemetrySnapshot {
        offset_min: 60,
        ..TelemetrySnapshot::default()
    });
    let snap = f.telemetry.snapshot(&f.jobs);
    assert_eq!(snap.limits[0].hits_at_clock.as_deref(), Some("13:00"));
    assert_eq!(snap.limits[1].projection, None);
    assert_eq!(f.store.limits().unwrap().len(), 2);
}

#[test]
fn stored_limits_and_environments_come_back_on_recover() {
    let f = fixture();
    f.store
        .save_limit(&limit("five_hour", 0.4, NOW + HOUR), NOW - MINUTE)
        .unwrap();
    let env = Environment {
        session_id: "s".into(),
        claude_code_version: Some("2.1.316".into()),
        mcp_servers: vec![McpServer {
            name: "docs".into(),
            status: "connected".into(),
        }],
        ..Environment::default()
    };
    f.store.save_environment(&env, NOW - MINUTE).unwrap();
    let mut jobs = f.jobs;
    jobs.recover().unwrap();
    let snap = f.telemetry.snapshot(&jobs);
    assert_eq!(snap.limits.len(), 1);
    assert_eq!(snap.limits[0].utilization, Some(0.4));
    assert_eq!(snap.environments[0].environment, env);
    assert_eq!(snap.environments[0].seen_at, NOW - MINUTE);
}

#[test]
fn environment_of_a_job_is_stored_and_listed() {
    let mut f = fixture();
    let job = f.jobs.dispatch(f.session.id, "go").unwrap();
    let env = Environment {
        session_id: f.session.id.to_string(),
        model: "claude-haiku-5-5".into(),
        ..Environment::default()
    };
    f.emit(AgentEvent::Environment(Box::new(env.clone())));
    assert_eq!(f.store.environments(5).unwrap()[0].0, env);
    let snap = f.telemetry.snapshot(&f.jobs);
    assert_eq!(snap.environments[0].environment, env);
    assert_eq!(
        f.jobs.job_activity(job).and_then(|a| a.environment.clone()),
        Some(env)
    );
}

#[test]
fn activity_view_has_flow_tools_ticker_and_files() {
    let mut f = fixture();
    let job = f.jobs.dispatch(f.session.id, "go").unwrap();
    f.emit(AgentEvent::ToolUse {
        id: "t1".into(),
        name: "Read".into(),
        summary: "src/lib.rs".into(),
        actor: Actor::Lead,
    });
    f.clock.advance(2_000);
    f.emit(AgentEvent::ToolResult {
        tool_use_id: "t1".into(),
        is_error: false,
        preview: "fn main".into(),
        actor: Actor::Lead,
    });
    for _ in 0..2 {
        // Claude Code repeats a message's usage on each content block.
        f.emit(AgentEvent::Usage {
            input: 10,
            output: 20,
            cache_read: 30,
            cache_creation: 0,
            actor: Actor::Lead,
        });
    }
    let snap = f.telemetry.snapshot(&f.jobs);
    let a = &snap.activity;
    assert_eq!(a.flow.len(), 60);
    assert_eq!(a.flow.iter().map(|b| b.output).sum::<u64>(), 20);
    assert_eq!(a.cache_hit_ratio, Some(0.75));
    assert_eq!(a.tools[0].name, "Read");
    assert_eq!(a.tools[0].avg_ms, Some(2_000));
    assert_eq!(a.ticker.len(), 1);
    assert_eq!(a.files[0].path, "src/lib.rs");
    let per_job = f.jobs.job_activity(job).unwrap();
    assert_eq!(
        per_job
            .flow
            .series(NOW + 2_000)
            .iter()
            .map(|b| b.input)
            .sum::<u64>(),
        10
    );
}

#[test]
fn todos_and_agent_tree_per_job() {
    use swarmux_domain::telemetry::{Todo, TodoStatus};
    let mut f = fixture();
    let job = f.jobs.dispatch(f.session.id, "go").unwrap();
    f.emit(AgentEvent::ToolUse {
        id: "task".into(),
        name: "Task".into(),
        summary: "explorer: map".into(),
        actor: Actor::Lead,
    });
    f.emit(AgentEvent::ToolUse {
        id: "g".into(),
        name: "Grep".into(),
        summary: "fn main".into(),
        actor: Actor::Subagent("task".into()),
    });
    let todos = vec![Todo {
        content: "Map".into(),
        status: TodoStatus::InProgress,
        active_form: Some("Mapping".into()),
    }];
    f.emit(AgentEvent::Todos {
        todos: todos.clone(),
        actor: Actor::Subagent("task".into()),
    });
    assert_eq!(f.jobs.job_activity(job).unwrap().todos, todos);
    let tree = f.jobs.agent_tree(job).unwrap();
    assert_eq!(tree.children[0].label, "explorer");
    assert_eq!(
        tree.children[0].current_tool.as_deref(),
        Some("Grep fn main")
    );
    assert_eq!(tree.current_tool.as_deref(), Some("Task explorer: map"));
    assert!(f.jobs.agent_tree(swarmux_domain::JobId(999)).is_none());
}

fn status(session: &str, at: i64, five: f64, resets: i64) -> Statusline {
    Statusline {
        session_id: session.into(),
        at,
        five_hour: Some(WindowReading {
            used: five,
            resets_at: Some(resets),
        }),
        ..Statusline::default()
    }
}

fn with_statusline(f: Fixture) -> (Fixture, Rc<FakeStatuslineFeed>, Rc<TruthBoard>) {
    let lines = Rc::new(FakeStatuslineFeed::default());
    let board = Rc::new(TruthBoard::new(f.clock.clone()));
    let telemetry = TelemetryService::new(f.feed.clone(), f.jobs_sessions(), f.clock.clone())
        .with_truth(board.clone(), lines.clone());
    (Fixture { telemetry, ..f }, lines, board)
}

impl Fixture {
    fn jobs_sessions(&self) -> Rc<dyn SessionRepository> {
        self.sessions.clone()
    }
}

#[test]
fn the_freshest_source_wins_each_window_and_is_marked() {
    let f = fixture();
    let reset = NOW + 2 * HOUR;
    let (mut f, lines, _) = with_statusline(f);
    f.jobs.dispatch(f.session.id, "go").unwrap();
    f.emit(AgentEvent::RateLimit(limit("five_hour", 0.5, reset)));
    f.emit(AgentEvent::RateLimit(limit(
        "seven_day",
        0.8,
        NOW + 3 * HOUR,
    )));
    let out = f.telemetry.snapshot(&f.jobs);
    assert!(
        out.limits
            .iter()
            .all(|l| l.source == LimitSource::RateLimitEvent)
    );

    // A status line reading taken after the stream's takes over its window only.
    lines.push(status("s1", NOW + 1000, 0.62, reset));
    let out = f.telemetry.snapshot(&f.jobs);
    let five = out.limits.iter().find(|l| l.window == "five_hour").unwrap();
    assert_eq!(five.source, LimitSource::Statusline);
    assert_eq!(five.utilization, Some(0.62));
    let seven = out.limits.iter().find(|l| l.window == "seven_day").unwrap();
    assert_eq!(seven.source, LimitSource::RateLimitEvent);
    assert_eq!(out.limits[0].window, "five_hour", "shortest window first");

    // An older status line reading never replaces a newer stream reading.
    lines.push(status("s2", NOW - 5000, 0.1, reset));
    let out = f.telemetry.snapshot(&f.jobs);
    let five = out.limits.iter().find(|l| l.window == "five_hour").unwrap();
    assert_eq!(five.utilization, Some(0.62));
}

#[test]
fn a_window_only_the_status_line_knows_is_listed() {
    let (f, lines, _) = with_statusline(fixture());
    lines.push(Statusline {
        seven_day: Some(WindowReading {
            used: 0.9,
            resets_at: Some(NOW + 3 * HOUR),
        }),
        ..status("s1", NOW, 0.1, NOW + HOUR)
    });
    let out = f.telemetry.snapshot(&f.jobs);
    let windows: Vec<_> = out.limits.iter().map(|l| l.window.as_str()).collect();
    assert_eq!(windows, ["five_hour", "seven_day"]);
    assert!(
        out.limits
            .iter()
            .all(|l| l.source == LimitSource::Statusline)
    );
    assert_eq!(out.limits[1].utilization, Some(0.9));
}

#[test]
fn cache_and_pull_request_come_from_the_status_line() {
    let (f, lines, _) = with_statusline(fixture());
    lines.push(Statusline {
        cache: Some(PromptCache {
            warm: Some(true),
            ttl: Some("5m".into()),
            expires_at: Some(NOW + 4 * MINUTE),
            hit_ratio: Some(0.93),
            ..PromptCache::default()
        }),
        pr: Some(PullRef {
            number: Some(7),
            review_state: Some("approved".into()),
            ..PullRef::default()
        }),
        ..status("s1", NOW - 1000, 0.1, NOW + HOUR)
    });
    lines.push(status("s2", NOW - 9000, 0.1, NOW + HOUR));
    let out = f.telemetry.snapshot(&f.jobs);
    assert_eq!(out.statuslines.len(), 2);
    let first = &out.statuslines[0];
    assert_eq!(first.session_id, "s1", "newest first");
    let cache = first.cache.as_ref().unwrap();
    assert!(cache.warm);
    assert_eq!(cache.expires_in, Some(4 * MINUTE));
    assert_eq!(cache.hit_ratio, Some(0.93));
    assert_eq!(
        first.pr.as_ref().unwrap().review_state.as_deref(),
        Some("approved")
    );

    // Five minutes later the cache has gone cold.
    f.clock.advance(5 * MINUTE);
    let cache = f.telemetry.snapshot(&f.jobs).statuslines[0]
        .cache
        .clone()
        .unwrap();
    assert!(!cache.warm);
    assert_eq!(cache.expires_in, Some(0));
}

#[test]
fn external_sessions_exclude_swarmux_jobs_and_keep_waiting_state() {
    let f = fixture();
    let ext = |id: &str, state, waiting: Option<&str>| ExternalSession {
        session_id: id.into(),
        name: id.into(),
        cwd: "/x".into(),
        kind: ExternalKind::Interactive,
        pid: Some(1),
        state,
        waiting_for: waiting.map(String::from),
        started_at: None,
    };
    let snap = TelemetrySnapshot {
        external: Collected {
            data: Some(vec![
                ext(&f.session.id.to_string(), ExternalState::Working, None),
                ext("outside", ExternalState::Blocked, Some("permission prompt")),
            ]),
            ..Collected::pending()
        },
        ..TelemetrySnapshot::default()
    };
    f.feed.set(snap);
    let out = f.telemetry.snapshot(&f.jobs);
    let listed = out.external.data.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].session_id, "outside");
    assert!(listed[0].is_waiting());
}

#[test]
fn instruction_inventory_is_in_the_snapshot() {
    let (f, _, board) = with_statusline(fixture());
    board.record_instruction(
        "s1",
        None,
        InstructionFile::from_notice("/w/CLAUDE.md", "session_start;Project", NOW),
    );
    let out = f.telemetry.snapshot(&f.jobs);
    assert_eq!(out.instructions.len(), 1);
    assert_eq!(out.instructions[0].files[0].memory_type, "Project");
}
