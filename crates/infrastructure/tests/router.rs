//! `HaikuRouter` against the fake `claude`. Run with `cargo test --features fake-claude`.
#![cfg(feature = "fake-claude")]

use std::sync::mpsc;
use std::time::Duration;

use swarmux_application::ports::{PromptRouter, RouteCandidate, RouteReply, RouteRequest};
use swarmux_infrastructure::HaikuRouter;

const FAKE: &str = env!("CARGO_BIN_EXE_fake_claude");

fn request() -> RouteRequest {
    let c = |name: &str| RouteCandidate {
        name: name.into(),
        cwd: format!("/w/{name}"),
        model: "sonnet".into(),
        recent: vec![],
    };
    RouteRequest {
        prompt: "fix the login button css".into(),
        candidates: vec![c("api"), c("web")],
    }
}

fn route(bin: &str, env: Vec<(String, String)>, timeout: Duration) -> RouteReply {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let router = HaikuRouter::new(bin.into(), rt.handle().clone())
        .with_env(env)
        .with_timeout(timeout);
    let (tx, rx) = mpsc::channel();
    router.route(
        request(),
        Box::new(move |r| {
            let _ = tx.send(r);
        }),
    );
    rx.recv_timeout(Duration::from_secs(20)).expect("a reply")
}

fn env(pairs: &[(&str, String)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

#[test]
fn routes_with_one_tool_less_haiku_call() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("out.json");
    let echo = dir.path().join("echo.json");
    std::fs::write(
        &fixture,
        r#"{"type":"result","is_error":false,"total_cost_usd":0.0002,"result":"{\"session\":\"web\",\"model\":\"haiku\",\"reason\":\"front-end change\"}"}"#,
    )
    .unwrap();
    let reply = route(
        FAKE,
        env(&[
            ("FAKE_CLAUDE_FIXTURE", fixture.display().to_string()),
            ("FAKE_CLAUDE_ECHO_FILE", echo.display().to_string()),
        ]),
        Duration::from_secs(20),
    );
    let answer = reply.result.unwrap();
    assert_eq!(answer.session, "web");
    assert_eq!(answer.model.as_deref(), Some("haiku"));
    assert_eq!(reply.cost_usd, 0.0002);

    let seen: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(echo).unwrap()).unwrap();
    let args: Vec<String> = serde_json::from_value(seen["args"].clone()).unwrap();
    let pair = |a: &str, b: &str| args.windows(2).any(|w| w[0] == a && w[1] == b);
    assert!(pair("--model", "haiku"), "{args:?}");
    assert!(pair("--tools", ""), "{args:?}");
    assert!(pair("--output-format", "json"), "{args:?}");
    assert!(
        seen["stdin"]
            .as_str()
            .unwrap()
            .contains("fix the login button css")
    );
    assert_eq!(seen["hook_pipe"], serde_json::Value::Null, "no guard hook");
}

#[test]
fn a_failing_claude_is_reported() {
    let reply = route(
        FAKE,
        env(&[
            ("FAKE_CLAUDE_EXIT", "1".into()),
            ("FAKE_CLAUDE_STDERR", "not logged in".into()),
        ]),
        Duration::from_secs(20),
    );
    let e = reply.result.unwrap_err();
    assert!(e.contains("code 1") && e.contains("not logged in"), "{e}");
}

#[test]
fn a_slow_claude_times_out() {
    let reply = route(
        FAKE,
        env(&[("FAKE_CLAUDE_SLEEP_MS", "5000".into())]),
        Duration::from_millis(300),
    );
    assert!(reply.result.unwrap_err().contains("no answer"));
}

#[test]
fn a_missing_claude_is_reported() {
    let reply = route("no-such-claude-binary", vec![], Duration::from_secs(5));
    assert!(reply.result.unwrap_err().contains("cannot run"));
}
