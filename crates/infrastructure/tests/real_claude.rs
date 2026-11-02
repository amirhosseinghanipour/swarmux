//! End-to-end check against the real `claude` CLI (uses your Claude plan; costs a few cents).
//! Run with: `cargo test -p swarmux-infrastructure --test real_claude -- --ignored`

use std::sync::mpsc;
use std::time::Duration;

use swarmux_application::ports::{AgentRunner, RunRequest, RunnerMsg};
use swarmux_domain::{AgentEvent, JobId, SessionId};
use swarmux_infrastructure::ClaudeCli;

#[test]
#[ignore = "runs the real claude CLI"]
fn real_claude_job_reports_init_and_result() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let bin = std::env::var("SWARMUX_CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
    let cli = ClaudeCli::new(bin.into(), rt.handle().clone());
    let (tx, rx) = mpsc::channel();
    cli.start(
        RunRequest {
            job_id: JobId(1),
            cwd: dir.path().into(),
            session_id: SessionId::random(),
            resume: false,
            fork: false,
            model: "haiku".into(),
            permission_mode: "default".into(),
            prompt: "Read a.txt with the Read tool, then reply with exactly the word in it.".into(),
            lean: false,
        },
        Box::new(move |m| {
            let _ = tx.send(m);
        }),
    );

    let mut saw_init = false;
    let mut result_text = None;
    while let Ok(m) = rx.recv_timeout(Duration::from_secs(180)) {
        match m {
            RunnerMsg::Event(AgentEvent::Init { .. }) => saw_init = true,
            RunnerMsg::Event(AgentEvent::Result {
                is_error, cost_usd, ..
            }) => result_text = Some((is_error, cost_usd)),
            RunnerMsg::Exited { .. } | RunnerMsg::SpawnFailed(_) => break,
            _ => {}
        }
    }
    assert!(saw_init, "no init event");
    let (is_error, cost) = result_text.expect("no result event");
    assert!(!is_error);
    assert!(cost > 0.0);
}
