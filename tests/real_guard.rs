//! The Guard against the real `claude` CLI (uses your Claude plan; costs a few cents).
//! Run with: `cargo test --test real_guard -- --ignored --nocapture`

use std::sync::mpsc;
use std::time::{Duration, Instant};

use swarmux_application::ports::{AgentRunner, HookInbox, RunRequest, RunnerMsg};
use swarmux_application::{GuardSettings, SharedPolicy};
use swarmux_domain::{AgentEvent, JobId, Pattern, SessionId, Unmatched, Verdict};
use swarmux_infrastructure::hook::endpoint_for;
use swarmux_infrastructure::{ClaudeCli, GuardHook, HookServer};

#[test]
#[ignore = "runs the real claude CLI"]
fn denied_bash_command_is_blocked_by_the_hook() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let settings = GuardSettings {
        deny: vec![Pattern::parse("Bash(echo swarmux-blocked-42)").unwrap()],
        unmatched: Unmatched::Pass,
        ..GuardSettings::default()
    };
    let pipe = endpoint_for(std::process::id());
    let server =
        HookServer::start(rt.handle(), &pipe, SharedPolicy::new(settings.policy())).unwrap();
    let bin = std::env::var("SWARMUX_CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
    let cli = ClaudeCli::new(bin.into(), rt.handle().clone()).with_guard(GuardHook {
        exe: env!("CARGO_BIN_EXE_swarmux").into(),
        pipe,
        timeout_secs: 30,
    });

    let dir = std::env::temp_dir().join(format!("swarmux-real-guard-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (tx, rx) = mpsc::channel();
    let start = Instant::now();
    cli.start(
        RunRequest {
            job_id: JobId(42),
            cwd: dir.clone(),
            session_id: SessionId::random(),
            resume: false,
            fork: false,
            model: "haiku".into(),
            permission_mode: "default".into(),
            prompt: "Use the Bash tool to run exactly this command once: echo swarmux-blocked-42 \
                     Do not retry it. Then reply with the error message you got, if any."
                .into(),
            lean: std::env::var("SWARMUX_REAL_NOT_LEAN").is_err(),
            review: false,
            env: Vec::new(),
            max_budget_usd: None,
            board: false,
            add_dirs: Vec::new(),
        },
        Box::new(move |m| {
            let _ = tx.send(m);
        }),
    );

    let mut denied_result = None;
    let mut init_at = None;
    while let Ok(m) = rx.recv_timeout(Duration::from_secs(240)) {
        match m {
            RunnerMsg::Event(AgentEvent::Init { .. }) => init_at = Some(start.elapsed()),
            RunnerMsg::Event(AgentEvent::ToolResult {
                is_error: true,
                preview,
                ..
            }) => denied_result = Some(preview),
            RunnerMsg::Exited { .. } | RunnerMsg::SpawnFailed(_) => break,
            _ => {}
        }
    }
    eprintln!(
        "init after {init_at:?}, finished after {:?}",
        start.elapsed()
    );
    let items = server.poll();
    eprintln!(
        "hook requests: {:?}",
        items
            .iter()
            .map(|i| (&i.request.tool_name, &i.request.subject, &i.answered))
            .collect::<Vec<_>>()
    );
    let item = items
        .iter()
        .find(|i| i.request.subject.contains("swarmux-blocked-42"))
        .expect("the hook never asked about the command");
    assert_eq!(item.request.job_id, Some(JobId(42)));
    assert_eq!(item.answered.as_ref().unwrap().0.verdict, Verdict::Deny);
    let preview = denied_result.expect("no failed tool result");
    eprintln!("claude saw: {preview}");
    assert!(preview.contains("swarmux guard denied it"), "{preview}");
    let _ = std::fs::remove_dir_all(dir);
}
