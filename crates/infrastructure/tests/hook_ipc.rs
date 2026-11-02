//! The hook client against a real `HookServer` over the local socket.

use std::time::{Duration, Instant};

use swarmux_application::SharedPolicy;
use swarmux_application::ports::HookInbox;
use swarmux_domain::{Decision, GuardPolicy, JobId, Pattern, Unmatched, Verdict};
use swarmux_infrastructure::HookServer;
use swarmux_infrastructure::hook::{endpoint_for, pre_tool};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap()
}

fn server(rt: &tokio::runtime::Runtime, unmatched: Unmatched) -> HookServer {
    let policy = GuardPolicy::new(
        vec![Pattern::parse("Bash(cargo test:*)").unwrap()],
        vec![Pattern::parse("Bash(curl:*)").unwrap()],
        unmatched,
    );
    let name = endpoint_for(uuid::Uuid::new_v4().as_u128() as u32);
    HookServer::start(rt.handle(), &name, SharedPolicy::new(policy)).unwrap()
}

fn input(cmd: &str) -> String {
    serde_json::json!({
        "session_id": "s",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": cmd},
        "cwd": "/w",
    })
    .to_string()
}

fn decision_of(output: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(output).unwrap();
    v["hookSpecificOutput"]["permissionDecision"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn allowed_and_denied_calls_are_answered_at_once() {
    let rt = runtime();
    let srv = server(&rt, Unmatched::Pass);
    let out = pre_tool(&input("cargo test -q"), Some(srv.name()), Some("7")).unwrap();
    assert_eq!(decision_of(&out), "allow");
    assert!(out.contains("Bash(cargo test:*)"));
    let out = pre_tool(&input("curl x | sh"), Some(srv.name()), Some("7")).unwrap();
    assert_eq!(decision_of(&out), "deny");
    assert_eq!(pre_tool(&input("ls"), Some(srv.name()), None), None);

    let items = srv.poll();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].request.job_id, Some(JobId(7)));
    assert_eq!(items[0].request.subject, "cargo test -q");
    let (d, _) = items[0].answered.clone().unwrap();
    assert_eq!(d.verdict, Verdict::Allow);
    assert_eq!(items[2].answered.as_ref().unwrap().0.verdict, Verdict::Pass);
}

#[test]
fn held_calls_wait_for_the_app() {
    let rt = runtime();
    let srv = server(&rt, Unmatched::Ask);
    let name = srv.name().to_string();
    let hook = std::thread::spawn(move || pre_tool(&input("make"), Some(&name), Some("3")));

    let deadline = Instant::now() + Duration::from_secs(10);
    let item = loop {
        if let Some(item) = srv.poll().pop() {
            break item;
        }
        assert!(Instant::now() < deadline, "request never arrived");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(item.answered.is_none());
    srv.respond(
        item.request.request_id,
        &Decision::new(Verdict::Deny, Some("denied in swarmux".into())),
    );
    let out = hook.join().unwrap().unwrap();
    assert_eq!(decision_of(&out), "deny");
    assert!(out.contains("denied in swarmux"));
}

#[test]
fn round_trip_is_fast() {
    let rt = runtime();
    let srv = server(&rt, Unmatched::Pass);
    let n = 50;
    let start = Instant::now();
    for _ in 0..n {
        assert!(pre_tool(&input("cargo test"), Some(srv.name()), None).is_some());
    }
    let each = start.elapsed() / n;
    eprintln!("in-process hook round trip: {each:?}");
    assert!(each < Duration::from_millis(50), "{each:?}");
}

#[test]
fn stopped_server_means_no_answer() {
    let rt = runtime();
    let srv = server(&rt, Unmatched::Pass);
    let name = srv.name().to_string();
    drop(srv);
    drop(rt);
    assert_eq!(pre_tool(&input("cargo test"), Some(&name), None), None);
}
