use std::process::Command;

use super::*;
use swarmux_application::Settings;

fn config(dir: &Path) -> CollectorConfig {
    let transcripts = dir.join("projects");
    std::fs::create_dir_all(transcripts.join("p")).unwrap();
    std::fs::write(
        transcripts.join("p").join("s.jsonl"),
        concat!(
            r#"{"type":"assistant","cwd":"/work","sessionId":"s1","message":{"id":"m1","model":"claude-haiku-4-5","usage":{"input_tokens":1,"output_tokens":1000000}},"requestId":"r1","timestamp":"2026-11-02T09:00:00Z"}"#,
            "\n"
        ),
    )
    .unwrap();
    CollectorConfig {
        settings: TelemetrySettings {
            usage_secs: 1,
            git_secs: 1,
            github_secs: 1,
            agents: false,
            ..TelemetrySettings::default()
        },
        pricing: PriceTable::builtin(),
        transcripts,
        usage_cache: Some(dir.join("usage.db")),
        home: dir.to_path_buf(),
        git: "git".into(),
        gh: dir.join("no-such-gh"),
        claude: dir.join("no-such-claude"),
    }
}

/// Polls `f` until it holds or 20 s pass.
fn eventually(c: &TelemetryCollector, f: impl Fn(&TelemetrySnapshot) -> bool) -> TelemetrySnapshot {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let snap = c.snapshot();
        if f(&snap) || Instant::now() > deadline {
            return snap;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn collectors_fill_the_snapshot_and_stop_on_drop() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let has_git = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .output()
        .is_ok_and(|o| o.status.success());
    let c = TelemetryCollector::start(config(dir.path()));
    c.focus(&repo);

    let snap = eventually(&c, |s| s.usage.data.is_some() && s.system.data.is_some());
    let usage = snap.usage.data.as_ref().unwrap();
    assert_eq!(usage.total.messages, 1);
    assert!(
        (usage.total.cost - 5.000001).abs() < 1e-9,
        "{}",
        usage.total.cost
    );
    assert!(snap.usage.updated_at.is_some());
    assert!(snap.system.data.as_ref().unwrap().mem_total > 0);

    let snap = eventually(&c, |s| s.github.hint.is_some());
    assert!(snap.github.data.is_none());
    assert!(snap.github.hint.as_deref().unwrap().contains("install"));
    if has_git {
        let snap = eventually(&c, |s| s.git.data.is_some());
        assert_eq!(snap.git.data.unwrap().cwd, repo.display().to_string());
    }

    // Moving the focus to a folder that isn't a repository clears git data.
    c.focus(dir.path());
    if has_git {
        let snap = eventually(&c, |s| s.git.data.is_none() && s.git.error.is_some());
        assert!(snap.git.hint.is_some(), "{:?}", snap.git);
    }
    let started = Instant::now();
    drop(c);
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn disabled_sources_stay_off_and_bad_paths_report_errors() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(dir.path());
    cfg.settings.system = false;
    cfg.settings.git = false;
    cfg.settings.github = false;
    cfg.transcripts = dir.path().join("missing");
    let c = TelemetryCollector::start(cfg);
    let snap = eventually(&c, |s| s.usage.error.is_some());
    assert!(!snap.system.enabled && !snap.git.enabled && !snap.github.enabled);
    assert!(snap.usage.enabled);
    assert!(
        snap.usage
            .hint
            .as_deref()
            .unwrap()
            .contains("transcripts_path")
    );
    assert!(snap.usage.data.is_none());
}

#[test]
fn default_transcripts_folder() {
    let home = Path::new("/home/u");
    if std::env::var_os("CLAUDE_CONFIG_DIR").is_none() {
        assert_eq!(
            default_transcripts(home),
            home.join(".claude").join("projects")
        );
    }
}

/// Runs every collector on this machine at the default cadences and prints
/// the process CPU use. `cargo test --release -- --ignored collector_overhead`
#[test]
#[ignore = "reads this machine's transcripts and takes over a minute"]
fn collector_overhead() {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let paths = Paths::default_home().unwrap();
    let settings = Settings::default();
    let c = TelemetryCollector::start(CollectorConfig::new(&settings, &paths));
    c.focus(Path::new(env!("CARGO_MANIFEST_DIR")));
    // Let the first transcript pass (from the cache) finish.
    eventually(&c, |s| {
        s.usage.data.is_some() && s.github.updated_at.is_some()
    });
    let me = Pid::from_u32(std::process::id());
    let mut sys = System::new().unwrap();
    let cpu_ms = |sys: &mut System| {
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[me]),
            true,
            ProcessRefreshKind::nothing().with_cpu(),
        );
        sys.process(me).map_or(0, |p| p.accumulated_cpu_time())
    };
    let before = cpu_ms(&mut sys);
    let started = Instant::now();
    std::thread::sleep(Duration::from_secs(60));
    let used = cpu_ms(&mut sys) - before;
    let wall = started.elapsed().as_millis() as u64;
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
    let snap = c.snapshot();
    eprintln!(
        "cpu {used} ms over {wall} ms wall: {:.2}% of one core, {:.3}% of {cores} cores; usage pass {:?} ms, system {:?} ms, git {:?} ms, gh {:?} ms",
        used as f64 * 100.0 / wall as f64,
        used as f64 * 100.0 / wall as f64 / cores,
        snap.usage.took_ms,
        snap.system.took_ms,
        snap.git.took_ms,
        snap.github.took_ms
    );
}

struct FakePoll(Result<Vec<swarmux_domain::telemetry::ExternalSession>, String>);

impl AgentsPoll for FakePoll {
    fn list(&self) -> Result<Vec<swarmux_domain::telemetry::ExternalSession>, String> {
        self.0.clone()
    }
}

fn shared() -> Shared {
    Shared {
        snap: Mutex::new(TelemetrySnapshot {
            external: Collected::pending(),
            ..TelemetrySnapshot::default()
        }),
        focus: Mutex::new((0, None)),
        stop: AtomicBool::new(false),
        bell: Condvar::new(),
        bell_lock: Mutex::new(()),
    }
}

#[test]
fn the_agents_worker_keeps_the_last_list_when_a_poll_fails() {
    use swarmux_domain::telemetry::{ExternalKind, ExternalSession, ExternalState};
    let sh = shared();
    let one = ExternalSession {
        session_id: "s".into(),
        name: "n".into(),
        cwd: "/w".into(),
        kind: ExternalKind::Background,
        pid: None,
        state: ExternalState::Blocked,
        waiting_for: Some("permission prompt".into()),
        started_at: None,
    };
    agents_worker(Box::new(FakePoll(Ok(vec![one.clone()]))))(&sh);
    let snap = lock(&sh.snap).clone();
    assert_eq!(snap.external.data, Some(vec![one.clone()]));
    assert!(snap.external.updated_at.is_some() && snap.external.error.is_none());

    agents_worker(Box::new(FakePoll(Err(
        "claude agents: not installed".into()
    ))))(&sh);
    let snap = lock(&sh.snap).clone();
    assert_eq!(snap.external.data, Some(vec![one]), "stale data is kept");
    assert_eq!(
        snap.external.error.as_deref(),
        Some("claude agents: not installed")
    );
}

#[test]
#[ignore = "runs the real claude CLI"]
fn the_real_claude_lists_its_sessions() {
    let sessions = agents::ClaudeAgents::new("claude".into()).list().unwrap();
    assert!(
        sessions.iter().all(|s| !s.session_id.is_empty()),
        "{sessions:?}"
    );
    eprintln!("{} sessions", sessions.len());
}
