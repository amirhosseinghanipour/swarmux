//! Every lifecycle event over the real local socket, client to server.

use std::io::{BufRead, BufReader, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use swarmux_application::ports::HookInbox;
use swarmux_application::{
    HookFeed, HookNotice, SessionStartContext, SessionStartInfo, SharedPolicy, StopAnswer,
    StopContext, StopPolicy,
};
use swarmux_domain::{GuardPolicy, HookEvent, JobId, Unmatched};
use swarmux_infrastructure::HookServer;
use swarmux_infrastructure::hook::{Responders, connect, endpoint_for, run_hook};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap()
}

fn policy() -> SharedPolicy {
    SharedPolicy::new(GuardPolicy::new(vec![], vec![], Unmatched::Pass))
}

fn name() -> String {
    endpoint_for(uuid::Uuid::new_v4().as_u128() as u32)
}

/// A hook input shaped like Claude Code 2.1's, per event.
fn input(event: HookEvent) -> String {
    let mut v = serde_json::json!({
        "session_id": "sess-1",
        "transcript_path": "/t.jsonl",
        "cwd": "/w",
        "hook_event_name": event.as_str(),
    });
    let extra = match event {
        HookEvent::PreToolUse => serde_json::json!({
            "tool_name": "Bash", "tool_input": {"command": "ls"}, "tool_use_id": "t1"}),
        HookEvent::PostToolUse => serde_json::json!({
            "tool_name": "Bash", "tool_input": {"command": "ls"},
            "tool_response": {"stdout": "x"}, "duration_ms": 12}),
        HookEvent::PostToolUseFailure => serde_json::json!({
            "tool_name": "Bash", "tool_input": {"command": "false"},
            "error": "exit code 1\nmore", "is_interrupt": false}),
        HookEvent::PermissionDenied => serde_json::json!({
            "tool_name": "Write", "tool_input": {"file_path": "/etc/x"},
            "reason": "[auto-mode] blocked"}),
        HookEvent::Stop | HookEvent::SubagentStop => serde_json::json!({
            "last_assistant_message": "All done.\nBye", "stop_hook_active": false}),
        HookEvent::SessionStart => serde_json::json!({"source": "startup"}),
        HookEvent::PreCompact => serde_json::json!({"trigger": "auto"}),
        HookEvent::Notification => serde_json::json!({
            "message": "Claude needs you", "notification_type": "agent_needs_input"}),
        HookEvent::StopFailure => serde_json::json!({"error": "rate_limit"}),
        HookEvent::InstructionsLoaded => serde_json::json!({
            "file_path": "/w/CLAUDE.md", "memory_type": "Project",
            "load_reason": "session_start"}),
        HookEvent::TaskCreated | HookEvent::TaskCompleted => serde_json::json!({
            "task_id": "task-001", "task_subject": "Write the migration",
            "task_description": "add the v15 table"}),
    };
    v.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    v.to_string()
}

fn wait_for(srv: &HookServer, n: usize) -> Vec<HookNotice> {
    let mut got = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while got.len() < n {
        got.extend(srv.poll_notices());
        assert!(
            Instant::now() < deadline,
            "only {} of {n} arrived",
            got.len()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    got
}

fn ask(srv: &HookServer, line: &str) -> String {
    let mut conn = connect(srv.name()).unwrap();
    conn.write_all(line.as_bytes()).unwrap();
    let mut answer = String::new();
    BufReader::new(conn).read_line(&mut answer).unwrap();
    answer
}

#[test]
fn every_lifecycle_event_round_trips() {
    let rt = runtime();
    let srv = HookServer::start(rt.handle(), &name(), policy()).unwrap();
    let events: Vec<_> = HookEvent::ALL
        .into_iter()
        .filter(|e| *e != HookEvent::PreToolUse)
        .collect();
    for event in &events {
        let out = run_hook(*event, &input(*event), Some(srv.name()), Some("7"));
        assert_eq!(out, None, "{event:?} prints nothing by default");
    }
    let mut got = wait_for(&srv, events.len());
    got.sort_by_key(|n| n.event.as_str());
    let mut want = events.clone();
    want.sort_by_key(|e| e.as_str());
    assert_eq!(got.iter().map(|n| n.event).collect::<Vec<_>>(), want);
    for n in &got {
        assert_eq!(n.job, Some(JobId(7)), "{:?}", n.event);
        assert_eq!(n.session, "sess-1");
        assert_eq!(n.input_sha256.len(), 64);
        assert_eq!(n.input_len, input(n.event).len());
        assert!(!n.summary.is_empty(), "{:?}", n.event);
    }
    let by = |e: HookEvent| got.iter().find(|n| n.event == e).unwrap();
    assert_eq!(by(HookEvent::PostToolUse).tool.as_deref(), Some("Bash"));
    assert_eq!(by(HookEvent::PostToolUse).summary, "Bash ls (12 ms)");
    assert_eq!(
        by(HookEvent::PermissionDenied).detail,
        "[auto-mode] blocked"
    );
    assert_eq!(by(HookEvent::StopFailure).detail, "rate_limit");
    assert_eq!(by(HookEvent::PreCompact).detail, "auto");
    assert_eq!(by(HookEvent::Notification).detail, "agent_needs_input");
    assert_eq!(by(HookEvent::Stop).summary, "All done.");
    assert_eq!(by(HookEvent::TaskCreated).summary, "Write the migration");
    assert_eq!(by(HookEvent::TaskCompleted).detail, "add the v15 table");
    assert_eq!(by(HookEvent::InstructionsLoaded).summary, "/w/CLAUDE.md");
    assert_eq!(
        by(HookEvent::InstructionsLoaded).detail,
        "session_start;Project"
    );
    assert!(
        by(HookEvent::PostToolUseFailure)
            .summary
            .ends_with("exit code 1")
    );
}

#[test]
fn pre_tool_use_is_still_the_guards() {
    let rt = runtime();
    let srv = HookServer::start(rt.handle(), &name(), policy()).unwrap();
    let out = run_hook(
        HookEvent::PreToolUse,
        &input(HookEvent::PreToolUse),
        Some(srv.name()),
        Some("2"),
    );
    assert_eq!(out, None); // Unmatched::Pass: leave it to Claude Code
    assert_eq!(srv.poll().len(), 1);
    assert!(srv.poll_notices().is_empty());
}

#[test]
fn a_request_in_the_first_hooks_format_still_works() {
    let rt = runtime();
    let srv = HookServer::start(rt.handle(), &name(), policy()).unwrap();
    let answer = ask(
        &srv,
        "{\"job_id\":5,\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"ls\"},\"cwd\":\"/w\"}\n",
    );
    assert_eq!(answer.trim(), r#"{"decision":"pass","reason":""}"#);
    let items = srv.poll();
    assert_eq!(items[0].request.job_id, Some(JobId(5)));
    assert_eq!(items[0].request.tool_name, "Bash");
}

#[test]
fn unknown_events_fail_open() {
    let rt = runtime();
    let srv = HookServer::start(rt.handle(), &name(), policy()).unwrap();
    // The client prints nothing for a name it doesn't know...
    assert_eq!(HookEvent::parse("UserPromptSubmit"), None);
    // ...and the server passes a request naming an event it doesn't know...
    let answer = ask(&srv, "{\"event\":\"FutureEvent\",\"raw\":\"{}\"}\n");
    assert!(answer.contains(r#""decision":"pass""#), "{answer}");
    // ...or one that isn't a request at all.
    let answer = ask(&srv, "not json\n");
    assert!(answer.contains(r#""decision":"pass""#), "{answer}");
    assert!(srv.poll_notices().is_empty());
    assert!(srv.poll().is_empty());
}

#[test]
fn unreachable_app_fails_open_for_every_event() {
    let name = name();
    for event in HookEvent::ALL {
        assert_eq!(run_hook(event, &input(event), Some(&name), None), None);
    }
}

#[test]
fn long_inputs_are_cut_but_still_hashed_whole() {
    let rt = runtime();
    let srv = HookServer::start(rt.handle(), &name(), policy()).unwrap();
    let big = serde_json::json!({
        "session_id": "s", "hook_event_name": "PostToolUse", "tool_name": "Read",
        "tool_input": {"file_path": "/a"}, "tool_response": "x".repeat(40_000),
    })
    .to_string();
    run_hook(HookEvent::PostToolUse, &big, Some(srv.name()), None);
    let n = wait_for(&srv, 1).remove(0);
    assert_eq!(n.input_len, big.len());
    assert_eq!(n.raw.len(), 16 * 1024);
    assert_eq!(
        n.input_sha256,
        swarmux_domain::ledger::hex(&swarmux_domain::ledger::sha256(big.as_bytes()))
    );
}

struct Blocker;

impl StopPolicy for Blocker {
    fn on_stop(&self, stop: &StopContext) -> StopAnswer {
        StopAnswer::Block {
            reason: format!(
                "not yet: {}",
                stop.last_message.lines().next().unwrap_or("")
            ),
        }
    }
}

struct Context;

impl SessionStartContext for Context {
    fn context(&self, start: &SessionStartInfo) -> Option<String> {
        Some(format!("source={}", start.source))
    }
}

#[test]
fn responders_answer_the_events_claude_code_waits_for() {
    let rt = runtime();
    let srv = HookServer::start_with(
        rt.handle(),
        &name(),
        policy(),
        Responders {
            stop: Arc::new(Blocker),
            session_start: Arc::new(Context),
            ..Responders::default()
        },
    )
    .unwrap();
    let out = run_hook(
        HookEvent::Stop,
        &input(HookEvent::Stop),
        Some(srv.name()),
        None,
    )
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["decision"], "block");
    assert_eq!(v["reason"], "not yet: All done.");
    let out = run_hook(
        HookEvent::SessionStart,
        &input(HookEvent::SessionStart),
        Some(srv.name()),
        None,
    )
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
    assert_eq!(
        v["hookSpecificOutput"]["additionalContext"],
        "source=startup"
    );
    // Asynchronous events are never answered.
    let out = run_hook(
        HookEvent::PreCompact,
        &input(HookEvent::PreCompact),
        Some(srv.name()),
        None,
    );
    assert_eq!(out, None);
}

#[test]
fn a_status_line_payload_reaches_the_app_and_the_line_is_printed() {
    use swarmux_application::StatuslineFeed;
    use swarmux_infrastructure::hook::run_statusline;

    let rt = runtime();
    let srv = HookServer::start(rt.handle(), &name(), policy()).unwrap();
    let payload = serde_json::json!({
        "session_id": "sess-9",
        "model": {"display_name": "Haiku"},
        "context_window": {"used_percentage": 20},
        "rate_limits": {
            "five_hour": {"used_percentage": 41, "resets_at": 1793620000},
            "seven_day": {"used_percentage": 90, "resets_at": 1794200000}
        },
        "prompt_cache": {"warm": true, "ttl": "5m", "hit_ratio": 0.9, "expires_at": 4102444800u64},
        "pr": {"number": 3, "review_state": "approved", "kind": "pr"},
    })
    .to_string();
    let line = run_statusline(
        &payload,
        Some(srv.name()),
        Some("7"),
        None,
        1_793_613_600_000,
    );
    assert!(
        line.starts_with("Haiku · ctx 20% · 5h 41% · 7d 90% · cache 90%"),
        "{line}"
    );
    assert!(line.ends_with("PR #3 approved"), "{line}");

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut got = Vec::new();
    while got.is_empty() {
        got.extend(srv.poll_statuslines());
        assert!(Instant::now() < deadline, "the payload never arrived");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].session_id, "sess-9");
    assert_eq!(got[0].job, Some(JobId(7)));
    assert_eq!(got[0].five_hour.map(|w| w.used), Some(0.41));
    assert!(srv.poll_notices().is_empty(), "not a lifecycle event");
    assert!(srv.poll().is_empty());
}
