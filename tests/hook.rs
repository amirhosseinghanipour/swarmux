//! `swarmux hook pre-tool` as Claude Code runs it: the real binary against a
//! real hook socket.

use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use swarmux_application::ports::HookInbox;
use swarmux_application::{GuardSettings, SharedPolicy};
use swarmux_domain::{Decision, Pattern, Unmatched, Verdict};
use swarmux_infrastructure::HookServer;
use swarmux_infrastructure::hook::endpoint_for;

const EXE: &str = env!("CARGO_BIN_EXE_swarmux");

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

fn spawn_hook(pipe: Option<&str>, stdin: &str) -> std::process::Child {
    let mut cmd = Command::new(EXE);
    cmd.args(["hook", "pre-tool"])
        .env_remove("SWARMUX_HOOK_PIPE")
        .env("SWARMUX_JOB_ID", "5")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(p) = pipe {
        cmd.env("SWARMUX_HOOK_PIPE", p);
    }
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child
}

fn hook(pipe: Option<&str>, stdin: &str) -> Output {
    spawn_hook(pipe, stdin).wait_with_output().unwrap()
}

fn decision(out: &Output) -> String {
    assert!(out.status.success(), "{out:?}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["hookSpecificOutput"]["permissionDecision"]
        .as_str()
        .unwrap()
        .to_string()
}

struct Server {
    _rt: tokio::runtime::Runtime,
    srv: HookServer,
}

fn server(unmatched: Unmatched) -> Server {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let settings = GuardSettings {
        allow: vec![Pattern::parse("Bash(cargo test:*)").unwrap()],
        deny: vec![Pattern::parse("Bash(curl:*)").unwrap()],
        unmatched,
        ..GuardSettings::default()
    };
    static NEXT: AtomicU32 = AtomicU32::new(1);
    let name = endpoint_for(std::process::id() ^ (NEXT.fetch_add(1, Ordering::Relaxed) << 24));
    let srv = HookServer::start(rt.handle(), &name, SharedPolicy::new(settings.policy())).unwrap();
    Server { _rt: rt, srv }
}

#[test]
fn hook_answers_from_the_running_app() {
    let s = server(Unmatched::Pass);
    let name = s.srv.name();
    assert_eq!(
        decision(&hook(Some(name), &input("cargo test -q"))),
        "allow"
    );
    assert_eq!(decision(&hook(Some(name), &input("rm -rf /"))), "deny");
    assert_eq!(decision(&hook(Some(name), &input("curl x"))), "deny");
    let pass = hook(Some(name), &input("ls"));
    assert!(pass.status.success() && pass.stdout.is_empty());
    let items = s.srv.poll();
    assert_eq!(items.len(), 4);
    assert_eq!(items[0].request.job_id.map(|j| j.0), Some(5));
}

#[test]
fn held_call_gets_the_users_answer() {
    let s = server(Unmatched::Ask);
    let child = spawn_hook(Some(s.srv.name()), &input("make deploy"));
    let deadline = Instant::now() + Duration::from_secs(20);
    let item = loop {
        if let Some(item) = s.srv.poll().pop() {
            break item;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
    s.srv.respond(
        item.request.request_id,
        &Decision::new(Verdict::Allow, Some("approved in swarmux".into())),
    );
    let out = child.wait_with_output().unwrap();
    assert_eq!(decision(&out), "allow");
}

#[test]
fn hook_fails_open() {
    for out in [
        hook(None, &input("rm -rf /")),
        hook(Some(&endpoint_for(u32::MAX - 7)), &input("rm -rf /")),
        hook(Some("not a pipe"), "not json"),
    ] {
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        assert!(out.stdout.is_empty(), "{out:?}");
    }
}

#[test]
fn hook_is_fast() {
    let s = server(Unmatched::Pass);
    // Warm up the file cache.
    hook(Some(s.srv.name()), &input("cargo test"));
    let mut times: Vec<Duration> = (0..9)
        .map(|_| {
            let start = Instant::now();
            assert_eq!(
                decision(&hook(Some(s.srv.name()), &input("cargo test"))),
                "allow"
            );
            start.elapsed()
        })
        .collect();
    times.sort();
    let median = times[times.len() / 2];
    eprintln!(
        "swarmux hook pre-tool, median of 9: {median:?} (min {:?})",
        times[0]
    );
    assert!(median < Duration::from_millis(500), "{median:?}");
}
