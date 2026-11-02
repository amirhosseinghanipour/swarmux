//! Job runner against the fake `claude`. Run with `cargo test --features fake-claude`.
#![cfg(feature = "fake-claude")]

use std::path::{Path, PathBuf};

use swarmux::claude::{JobSpec, RunnerMsg, build_args, run_job, user_message};
use tokio::sync::mpsc;
use uuid::Uuid;

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

fn spec(cwd: PathBuf, env: &[(&str, String)]) -> JobSpec {
    JobSpec {
        claude_bin: env!("CARGO_BIN_EXE_fake_claude").into(),
        cwd,
        session_id: Uuid::new_v4(),
        resume: false,
        fork: false,
        model: "sonnet".into(),
        permission_mode: "default".into(),
        env: env
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    }
}

async fn run(spec: JobSpec, prompt: &str) -> Vec<RunnerMsg> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    run_job(spec, prompt.to_string(), tx).await;
    let mut out = Vec::new();
    while let Ok(m) = rx.try_recv() {
        out.push(m);
    }
    out
}

#[tokio::test]
async fn streams_fixture_events_then_exits() {
    let dir = tempfile::tempdir().unwrap();
    let msgs = run(
        spec(
            dir.path().into(),
            &[("FAKE_CLAUDE_FIXTURE", fixture("stream_basic.jsonl"))],
        ),
        "hi",
    )
    .await;
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

#[tokio::test]
async fn passes_prompt_args_and_cwd_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().join("with space");
    std::fs::create_dir(&cwd).unwrap();
    let echo = dir.path().join("echo.json");
    let s = spec(
        cwd,
        &[("FAKE_CLAUDE_ECHO_FILE", echo.to_string_lossy().into_owned())],
    );
    let prompt = "say \"hi\"\nnext — ñ 🐝";
    run(s.clone(), prompt).await;

    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&echo).unwrap()).unwrap();
    assert_eq!(v["stdin"], format!("{}\n", user_message(prompt)));
    assert!(v["cwd"].as_str().unwrap().ends_with("with space"));
    let args: Vec<String> = serde_json::from_value(v["args"].clone()).unwrap();
    assert_eq!(args, build_args(&s));
}

#[tokio::test]
async fn nonzero_exit_reports_stderr_tail() {
    let dir = tempfile::tempdir().unwrap();
    let msgs = run(
        spec(
            dir.path().into(),
            &[
                ("FAKE_CLAUDE_EXIT", "3".into()),
                ("FAKE_CLAUDE_STDERR", "boom".into()),
            ],
        ),
        "hi",
    )
    .await;
    assert_eq!(
        msgs.last(),
        Some(&RunnerMsg::Exited {
            code: Some(3),
            stderr_tail: "boom".into()
        })
    );
}

#[tokio::test]
async fn spawn_failure_reports_failed() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = spec(dir.path().into(), &[]);
    s.claude_bin = "definitely-not-a-binary-xyz".into();
    let msgs = run(s, "hi").await;
    assert_eq!(msgs.len(), 1);
    assert!(matches!(msgs[0], RunnerMsg::SpawnFailed(_)));
}
