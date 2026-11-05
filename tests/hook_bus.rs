//! The hook bus end to end: the real `swarmux hook <event>` binary, a real
//! hook socket, the bus and a real SQLite ledger.
//!
//! The ignored test at the bottom does the same with the real `claude`
//! (uses your Claude plan; a few cents):
//! `cargo test --test hook_bus -- --ignored --nocapture`

use std::io::Write;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use swarmux_application::ledger::{LedgerService, tail, verify_all};
use swarmux_application::ports::{AgentRunner, RunRequest, RunnerMsg};
use swarmux_application::{GuardSettings, HookBus, SharedPolicy};
use swarmux_domain::ledger::LedgerKind;
use swarmux_domain::{JobId, SessionId};
use swarmux_infrastructure::hook::endpoint_for;
use swarmux_infrastructure::{
    ClaudeCli, GuardHook, HookServer, SqliteLedger, SqliteStore, SystemClock,
};

const EXE: &str = env!("CARGO_BIN_EXE_swarmux");

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

/// Runs `swarmux hook <arg>` as Claude Code would.
fn hook(pipe: &str, arg: &str, stdin: &str) -> std::process::Output {
    let mut child = Command::new(EXE)
        .args(["hook", arg])
        .env("SWARMUX_HOOK_PIPE", pipe)
        .env("SWARMUX_JOB_ID", "9")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn bus(server: &Rc<HookServer>) -> (HookBus, Rc<SqliteLedger>) {
    let ledger = Rc::new(SqliteLedger::open_in_memory().unwrap());
    let clock = Rc::new(SystemClock);
    let service = Rc::new(LedgerService::new(ledger.clone(), clock.clone()));
    let store = Rc::new(SqliteStore::open_in_memory().unwrap());
    (HookBus::new(server.clone(), service, store, clock), ledger)
}

fn events_in(ledger: &SqliteLedger) -> Vec<String> {
    tail(ledger, 100)
        .unwrap()
        .into_iter()
        .filter(|e| e.draft.kind == LedgerKind::AgentEvent && e.draft.actor == "hook")
        .map(|e| e.draft.payload_json)
        .collect()
}

#[test]
fn real_hook_processes_land_in_the_ledger() {
    let rt = runtime();
    let pipe = endpoint_for(std::process::id() ^ 0x5a5a_0000);
    let policy = SharedPolicy::new(GuardSettings::default().policy());
    let server = Rc::new(HookServer::start(rt.handle(), &pipe, policy).unwrap());
    let (mut bus, ledger) = bus(&server);

    // Inputs as Claude Code 2.1 gives them (captured shapes, values made up).
    let common = |event: &str| {
        serde_json::json!({
            "session_id": "00000000-0000-4000-8000-000000000001",
            "transcript_path": "/t.jsonl",
            "cwd": "/w",
            "hook_event_name": event,
        })
    };
    let with = |event: &str, extra: serde_json::Value| {
        let mut v = common(event);
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        v.to_string()
    };
    let cases = [
        (
            "session-start",
            with("SessionStart", serde_json::json!({"source": "startup"})),
        ),
        (
            "post-tool-use",
            with(
                "PostToolUse",
                serde_json::json!({"tool_name": "Bash", "tool_input": {"command": "echo hi"},
                    "tool_response": {"stdout": "hi"}, "tool_use_id": "t"}),
            ),
        ),
        (
            "Stop",
            with(
                "Stop",
                serde_json::json!({"stop_hook_active": false, "last_assistant_message": "ok",
                    "background_tasks": [], "session_crons": []}),
            ),
        ),
    ];
    for (arg, input) in &cases {
        let out = hook(&pipe, arg, input);
        assert!(out.status.success(), "{out:?}");
        assert!(out.stdout.is_empty(), "{arg} printed {:?}", out.stdout);
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    while events_in(&ledger).len() < cases.len() {
        assert!(bus.pump().is_empty());
        assert!(Instant::now() < deadline, "events never reached the ledger");
        std::thread::sleep(Duration::from_millis(10));
    }
    let events = events_in(&ledger);
    for name in ["SessionStart", "PostToolUse", "Stop"] {
        assert!(
            events
                .iter()
                .any(|p| p.contains(&format!(r#""event":"{name}""#))),
            "{name} missing from {events:?}"
        );
    }
    assert_eq!(
        bus.signals(JobId(9)).map(|s| s.last_tool.clone()),
        Some(Some("Bash".into()))
    );
    assert!(verify_all(&*ledger).unwrap().is_ok());
}

#[test]
#[ignore = "runs the real claude CLI"]
fn a_real_job_records_session_start_tool_use_and_stop() {
    let rt = runtime();
    let pipe = endpoint_for(std::process::id());
    let policy = SharedPolicy::new(GuardSettings::default().policy());
    let server = Rc::new(HookServer::start(rt.handle(), &pipe, policy).unwrap());
    let (mut bus, ledger) = bus(&server);
    let bin = std::env::var("SWARMUX_CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
    let cli = ClaudeCli::new(bin.into(), rt.handle().clone()).with_guard(GuardHook {
        exe: EXE.into(),
        pipe,
        timeout_secs: 30,
    });
    let dir = std::env::temp_dir().join(format!("swarmux-real-bus-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (tx, rx) = mpsc::channel();
    cli.start(
        RunRequest {
            job_id: JobId(9),
            cwd: dir.clone(),
            session_id: SessionId::random(),
            resume: false,
            fork: false,
            model: "haiku".into(),
            permission_mode: "bypassPermissions".into(),
            prompt: "Run the Bash command `echo hi`, then reply with the word ok.".into(),
            lean: true,
            review: false,
            env: Vec::new(),
            max_budget_usd: None,
            board: false,
        },
        Box::new(move |m| {
            let _ = tx.send(m);
        }),
    );
    let mut states = Vec::new();
    while let Ok(m) = rx.recv_timeout(Duration::from_secs(240)) {
        match m {
            RunnerMsg::Event(swarmux_domain::AgentEvent::SessionState(s)) => states.push(s),
            RunnerMsg::Exited { .. } | RunnerMsg::SpawnFailed(_) => break,
            _ => {}
        }
        bus.pump();
    }
    std::thread::sleep(Duration::from_millis(300));
    bus.pump();
    let events = events_in(&ledger);
    eprintln!("session states: {states:?}\nhook events: {events:#?}");
    for name in ["SessionStart", "PostToolUse", "Stop"] {
        assert!(
            events
                .iter()
                .any(|p| p.contains(&format!(r#""event":"{name}""#))),
            "{name} missing"
        );
    }
    assert!(!states.is_empty(), "no session_state_changed events");
    let _ = std::fs::remove_dir_all(dir);
}
