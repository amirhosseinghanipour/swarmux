//! `ClaudeCli` against the fake `claude`. Run with `cargo test --features fake-claude`.
#![cfg(feature = "fake-claude")]

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use swarmux_application::ports::{AgentRunner, RunRequest, RunnerMsg};
use swarmux_domain::{JobId, SessionId};
use swarmux_infrastructure::claude_cli::command::{build_args, user_message};
use swarmux_infrastructure::{ClaudeCli, GuardHook};

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

fn request(cwd: PathBuf, prompt: &str) -> RunRequest {
    RunRequest {
        job_id: JobId(1),
        cwd,
        session_id: SessionId::random(),
        resume: false,
        fork: false,
        model: "sonnet".into(),
        permission_mode: "default".into(),
        prompt: prompt.into(),
        lean: false,
        review: false,
        env: Vec::new(),
        max_budget_usd: None,
        board: false,
        add_dirs: Vec::new(),
    }
}

/// Runs one request through the adapter and collects every message.
fn run(bin: &str, env: &[(&str, String)], req: RunRequest) -> Vec<RunnerMsg> {
    run_with(bin, env, None, req)
}

fn run_with(
    bin: &str,
    env: &[(&str, String)],
    guard: Option<GuardHook>,
    req: RunRequest,
) -> Vec<RunnerMsg> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let env = env
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    let mut cli = ClaudeCli::new(bin.into(), rt.handle().clone()).with_env(env);
    if let Some(guard) = guard {
        cli = cli.with_guard(guard);
    }
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
fn guard_hook_adds_settings_and_env() {
    let dir = tempfile::tempdir().unwrap();
    let echo = dir.path().join("echo.json");
    let guard = GuardHook {
        exe: r"C:\bin\swarmux.exe".into(),
        pipe: r"\\.\pipe\swarmux-test".into(),
        timeout_secs: 130,
    };
    let req = request(dir.path().into(), "hi");
    run_with(
        FAKE,
        &[("FAKE_CLAUDE_ECHO_FILE", echo.to_string_lossy().into_owned())],
        Some(guard.clone()),
        req.clone(),
    );
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&echo).unwrap()).unwrap();
    let args: Vec<String> = serde_json::from_value(v["args"].clone()).unwrap();
    assert_eq!(args[..args.len() - 2], build_args(&req));
    assert_eq!(args[args.len() - 2], "--settings");
    let settings: serde_json::Value = serde_json::from_str(&args[args.len() - 1]).unwrap();
    let hook = &settings["hooks"]["PreToolUse"][0]["hooks"][0];
    assert_eq!(hook["command"], r"C:\bin\swarmux.exe");
    assert_eq!(hook["timeout"], 130);
    assert_eq!(v["hook_pipe"], guard.pipe);
    assert_eq!(v["job_id"], req.job_id.to_string());
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

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap()
}

/// Waits up to `secs` for `cond`.
fn eventually(secs: u64, cond: impl Fn() -> bool) -> bool {
    let end = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < end {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    cond()
}

#[test]
fn finished_job_is_forgotten() {
    let rt = runtime();
    let cli = ClaudeCli::new(FAKE.into(), rt.handle().clone());
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::channel();
    cli.start(
        request(dir.path().into(), "hi"),
        Box::new(move |m| {
            let _ = tx.send(m);
        }),
    );
    assert_eq!(cli.running(), 1);
    while let Ok(m) = rx.recv_timeout(Duration::from_secs(20)) {
        if matches!(m, RunnerMsg::Exited { .. }) {
            break;
        }
    }
    assert!(eventually(5, || cli.running() == 0));
}

#[test]
fn cancel_kills_the_process() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let done = dir.path().join("done");
    let env = vec![
        ("FAKE_CLAUDE_SLEEP_MS".to_string(), "1500".to_string()),
        (
            "FAKE_CLAUDE_DONE_FILE".to_string(),
            done.to_string_lossy().into_owned(),
        ),
    ];
    let cli = ClaudeCli::new(FAKE.into(), rt.handle().clone()).with_env(env);
    let (tx, rx) = mpsc::channel();
    let req = request(dir.path().into(), "hi");
    let job = req.job_id;
    cli.start(
        req,
        Box::new(move |m| {
            let _ = tx.send(m);
        }),
    );
    std::thread::sleep(Duration::from_millis(300));
    cli.cancel(job);
    assert_eq!(cli.running(), 0);
    cli.cancel(JobId(12345)); // unknown jobs are ignored

    std::thread::sleep(Duration::from_millis(2500));
    assert!(!done.exists(), "the process was not killed");
    assert!(
        rx.try_iter()
            .all(|m| !matches!(m, RunnerMsg::Exited { .. })),
        "a cancelled job reports nothing more"
    );
}

#[test]
fn review_runs_read_only_and_answer_with_a_verdict() {
    use swarmux_domain::{AgentEvent, ReviewVerdict, parse_review};
    let dir = tempfile::tempdir().unwrap();
    let echo = dir.path().join("echo.json");
    let req = RunRequest {
        review: true,
        env: Vec::new(),
        max_budget_usd: None,
        board: false,
        model: "haiku".into(),
        ..request(dir.path().into(), "swarmux review of job #1")
    };
    let msgs = run(
        FAKE,
        &[
            ("FAKE_CLAUDE_ECHO_FILE", echo.to_string_lossy().into_owned()),
            ("FAKE_CLAUDE_FIXTURE", fixture("stream_review.jsonl")),
        ],
        req.clone(),
    );
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&echo).unwrap()).unwrap();
    let args: Vec<String> = serde_json::from_value(v["args"].clone()).unwrap();
    assert_eq!(args, build_args(&req));
    let pair = |a: &str, b: &str| args.windows(2).any(|w| w[0] == a && w[1] == b);
    assert!(pair("--tools", "Read,Grep,Glob") && pair("--model", "haiku"));
    assert!(!args.iter().any(|a| a == "--session-id" || a == "--resume"));

    let text = msgs
        .iter()
        .find_map(|m| match m {
            RunnerMsg::Event(AgentEvent::Result { text, cost_usd, .. }) => {
                Some((text.clone(), *cost_usd))
            }
            _ => None,
        })
        .unwrap();
    let review = parse_review(&text.0).unwrap();
    assert_eq!(
        (review.verdict, review.diffs_checked),
        (ReviewVerdict::Pass, 2)
    );
    assert_eq!(text.1, 0.0042);
}

#[test]
fn command_line_shows_the_real_flags_without_the_hook_json() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let cli = ClaudeCli::new(FAKE.into(), rt.handle().clone()).with_guard(GuardHook {
        exe: "swarmux".into(),
        pipe: "p".into(),
        timeout_secs: 10,
    });
    let line = cli
        .command_line(&request(PathBuf::from("/w"), "hi"))
        .unwrap();
    assert!(
        line.starts_with("claude -p --output-format stream-json"),
        "{line}"
    );
    assert!(
        line.contains("--model sonnet --permission-mode default"),
        "{line}"
    );
    assert!(line.ends_with("--settings …"), "{line}");
}

/// The process ids a fake `claude` and its grandchildren wrote down.
fn recorded_pids(file: &Path) -> Vec<u32> {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

fn is_running(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let mut sys = System::new().unwrap();
    let pid = Pid::from_u32(pid);
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid).is_some()
}

/// Starts a job whose `claude` has two grandchildren, cancels it, and
/// expects no process of the tree to remain.
fn cancel_leaves_no_process(bin: &Path) {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let pids = dir.path().join("pids");
    let env = vec![
        ("FAKE_CLAUDE_SLEEP_MS".to_string(), "120000".to_string()),
        ("FAKE_CLAUDE_GRANDCHILDREN".to_string(), "2".to_string()),
        (
            "FAKE_CLAUDE_PIDS_FILE".to_string(),
            pids.to_string_lossy().into_owned(),
        ),
    ];
    let cli = ClaudeCli::new(bin.into(), rt.handle().clone()).with_env(env);
    let req = request(dir.path().into(), "hi");
    let job = req.job_id;
    cli.start(req, Box::new(|_| {}));
    assert!(
        eventually(30, || recorded_pids(&pids).len() >= 3),
        "claude and its two grandchildren did not all start: {:?}",
        recorded_pids(&pids)
    );
    let tree = recorded_pids(&pids);
    assert!(tree.iter().all(|p| is_running(*p)));

    cli.cancel(job);
    assert!(
        eventually(30, || tree.iter().all(|p| !is_running(*p))),
        "still running after cancel: {:?}",
        tree.iter().filter(|p| is_running(**p)).collect::<Vec<_>>()
    );
}

#[test]
fn cancel_kills_every_descendant() {
    cancel_leaves_no_process(Path::new(FAKE));
}

/// `claude` on Windows is usually an npm `.cmd` wrapper around `node`.
#[cfg(windows)]
#[test]
fn cancel_kills_every_descendant_behind_a_cmd_wrapper() {
    let dir = tempfile::tempdir().unwrap();
    let wrapper = dir.path().join("claude.cmd");
    std::fs::write(&wrapper, format!("@echo off\r\n\"{FAKE}\" %*\r\n")).unwrap();
    cancel_leaves_no_process(&wrapper);
}

#[test]
fn eight_concurrent_starts_are_spaced_apart() {
    use swarmux_infrastructure::process::Stagger;
    const GAP: Duration = Duration::from_millis(600);
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let starts = dir.path().join("starts");
    let env = vec![(
        "FAKE_CLAUDE_START_FILE".to_string(),
        starts.to_string_lossy().into_owned(),
    )];
    let stagger = std::sync::Arc::new(Stagger::new(GAP, Duration::from_millis(50)));
    let cli = ClaudeCli::new(FAKE.into(), rt.handle().clone())
        .with_env(env)
        .with_stagger(stagger);
    let (tx, rx) = mpsc::channel();
    for n in 1..=8 {
        let mut req = request(dir.path().into(), "hi");
        req.job_id = JobId(n);
        let tx = tx.clone();
        cli.start(
            req,
            Box::new(move |m| {
                let _ = tx.send(m);
            }),
        );
    }
    let mut exited = 0;
    while exited < 8 {
        match rx.recv_timeout(Duration::from_secs(60)) {
            Ok(RunnerMsg::Exited { .. }) => exited += 1,
            Ok(_) => {}
            Err(e) => panic!("only {exited} of 8 jobs finished: {e}"),
        }
    }
    let mut at: Vec<u128> = std::fs::read_to_string(&starts)
        .unwrap()
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect();
    assert_eq!(at.len(), 8, "{at:?}");
    at.sort_unstable();
    // Process start-up time varies, so only a third of the gap is required.
    let least = (GAP / 3).as_nanos();
    for pair in at.windows(2) {
        let spacing = pair[1] - pair[0];
        assert!(
            spacing >= least,
            "two claude processes started {} ms apart",
            spacing / 1_000_000
        );
    }
    let span = (at[7] - at[0]) as f64 / 1e9;
    assert!(span >= 7.0 * GAP.as_secs_f64() * 0.6, "all within {span} s");
}

#[test]
fn a_start_that_collides_on_the_config_file_is_retried() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let counter = dir.path().join("count");
    let env = vec![
        ("FAKE_CLAUDE_CONTENTION".to_string(), "2".to_string()),
        (
            "FAKE_CLAUDE_COUNTER_FILE".to_string(),
            counter.to_string_lossy().into_owned(),
        ),
        (
            "FAKE_CLAUDE_FIXTURE".to_string(),
            fixture("stream_basic.jsonl"),
        ),
    ];
    let msgs = run_with_cli(
        ClaudeCli::new(FAKE.into(), rt.handle().clone()).with_env(env),
        request(dir.path().into(), "hi"),
    );
    let tries = std::fs::read_to_string(&counter).unwrap().lines().count();
    assert_eq!(tries, 3, "two collisions, then a run");
    assert!(
        matches!(msgs.last(), Some(RunnerMsg::Exited { code: Some(0), .. })),
        "{msgs:?}"
    );
}

#[test]
fn a_start_that_keeps_colliding_reports_the_failure() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let counter = dir.path().join("count");
    let env = vec![
        ("FAKE_CLAUDE_CONTENTION".to_string(), "99".to_string()),
        (
            "FAKE_CLAUDE_COUNTER_FILE".to_string(),
            counter.to_string_lossy().into_owned(),
        ),
    ];
    let msgs = run_with_cli(
        ClaudeCli::new(FAKE.into(), rt.handle().clone()).with_env(env),
        request(dir.path().into(), "hi"),
    );
    let tries = std::fs::read_to_string(&counter).unwrap().lines().count();
    assert_eq!(tries, 4, "one start and three retries");
    match msgs.last() {
        Some(RunnerMsg::Exited { code, stderr_tail }) => {
            assert_eq!(*code, Some(1));
            assert!(stderr_tail.contains(".claude.json"), "{stderr_tail}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_ordinary_failure_is_not_retried() {
    let rt = runtime();
    let dir = tempfile::tempdir().unwrap();
    let counter = dir.path().join("count");
    let env = vec![
        ("FAKE_CLAUDE_EXIT".to_string(), "1".to_string()),
        (
            "FAKE_CLAUDE_STDERR".to_string(),
            "Invalid API key".to_string(),
        ),
        ("FAKE_CLAUDE_CONTENTION".to_string(), "0".to_string()),
        (
            "FAKE_CLAUDE_COUNTER_FILE".to_string(),
            counter.to_string_lossy().into_owned(),
        ),
    ];
    let msgs = run_with_cli(
        ClaudeCli::new(FAKE.into(), rt.handle().clone()).with_env(env),
        request(dir.path().into(), "hi"),
    );
    assert_eq!(
        std::fs::read_to_string(&counter).unwrap().lines().count(),
        1
    );
    assert!(matches!(
        msgs.last(),
        Some(RunnerMsg::Exited { code: Some(1), .. })
    ));
}

/// Like `run_with` for a runner that is already built.
fn run_with_cli(cli: ClaudeCli, req: RunRequest) -> Vec<RunnerMsg> {
    let (tx, rx) = mpsc::channel();
    cli.start(req, Box::new(move |m| tx.send(m).unwrap()));
    let mut out = Vec::new();
    while let Ok(m) = rx.recv_timeout(Duration::from_secs(60)) {
        let done = matches!(m, RunnerMsg::Exited { .. } | RunnerMsg::SpawnFailed(_));
        out.push(m);
        if done {
            break;
        }
    }
    out
}
