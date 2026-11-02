use std::process::Command;

use super::*;

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
            ..TelemetrySettings::default()
        },
        pricing: PriceTable::builtin(),
        transcripts,
        usage_cache: Some(dir.join("usage.db")),
        home: dir.to_path_buf(),
        git: "git".into(),
        gh: dir.join("no-such-gh"),
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
