//! `ClaudeCli` against the fake `claude`. Run with `cargo test --features fake-claude`.
#![cfg(feature = "fake-claude")]

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use swarmux_application::ports::{AgentRunner, RunRequest, RunnerMsg};
use swarmux_domain::SessionId;
use swarmux_infrastructure::ClaudeCli;
use swarmux_infrastructure::claude_cli::command::{build_args, user_message};

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

fn request(cwd: PathBuf, prompt: &str) -> RunRequest {
    RunRequest {
        cwd,
        session_id: SessionId::random(),
        resume: false,
        fork: false,
        model: "sonnet".into(),
        permission_mode: "default".into(),
        prompt: prompt.into(),
    }
}

/// Runs one request through the adapter and collects every message.
fn run(bin: &str, env: &[(&str, String)], req: RunRequest) -> Vec<RunnerMsg> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let env = env
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    let cli = ClaudeCli::new(bin.into(), rt.handle().clone()).with_env(env);
    let (tx, rx) = mpsc::channel();
    cli.start(req, Box::new(move |m| tx.send(m).unwrap()));
    let mut out = Vec::new();
    while let Ok(m) = rx.recv_timeout(Duration::from_secs(20)) {
        let done = matches!(m, RunnerMsg::Exited { .. } | RunnerMsg::SpawnFailed(_));
        out.push(m);
        if done {
            break;
        }
    }
    out
}

const FAKE: &str = env!("CARGO_BIN_EXE_fake_claude");

#[test]
fn streams_fixture_events_then_exits() {
    let dir = tempfile::tempdir().unwrap();
    let msgs = run(
        FAKE,
        &[("FAKE_CLAUDE_FIXTURE", fixture("stream_basic.jsonl"))],
        request(dir.path().into(), "hi"),
    );
    let kinds: Vec<&str> = msgs
        .iter()
        .filter_map(|m| match m {
            RunnerMsg::Event(e) => Some(e.kind()),
            _ => None,
        })
        .collect();
    assert!(kinds.contains(&"init") && kinds.contains(&"result"));
    assert_eq!(
        msgs.last(),
        Some(&RunnerMsg::Exited {
            code: Some(0),
            stderr_tail: String::new()
        })
    );
}

#[test]
fn passes_prompt_args_and_cwd_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().join("with space");
    std::fs::create_dir(&cwd).unwrap();
    let echo = dir.path().join("echo.json");
    let prompt = "say \"hi\"\nnext — ñ 🐝";
    let req = request(cwd, prompt);
    run(
        FAKE,
        &[("FAKE_CLAUDE_ECHO_FILE", echo.to_string_lossy().into_owned())],
        req.clone(),
    );

    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&echo).unwrap()).unwrap();
    assert_eq!(v["stdin"], format!("{}\n", user_message(prompt)));
    assert!(v["cwd"].as_str().unwrap().ends_with("with space"));
    let args: Vec<String> = serde_json::from_value(v["args"].clone()).unwrap();
    assert_eq!(args, build_args(&req));
}

#[test]
fn nonzero_exit_reports_stderr_tail() {
    let dir = tempfile::tempdir().unwrap();
    let msgs = run(
        FAKE,
        &[
            ("FAKE_CLAUDE_EXIT", "3".into()),
            ("FAKE_CLAUDE_STDERR", "boom".into()),
        ],
        request(dir.path().into(), "hi"),
    );
    assert_eq!(
        msgs.last(),
        Some(&RunnerMsg::Exited {
            code: Some(3),
            stderr_tail: "boom".into()
        })
    );
}

#[test]
fn spawn_failure_reports_failed() {
    let dir = tempfile::tempdir().unwrap();
    let msgs = run(
        "definitely-not-a-binary-xyz",
        &[],
        request(dir.path().into(), "hi"),
    );
    assert_eq!(msgs.len(), 1);
    assert!(matches!(msgs[0], RunnerMsg::SpawnFailed(_)));
}
