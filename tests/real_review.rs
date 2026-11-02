//! A whole job against the real `claude` CLI: a Haiku lead that delegates to
//! one subagent, the Guard hook, then a Haiku review (uses your Claude plan;
//! costs a few cents). Run with:
//! `cargo test --test real_review -- --ignored --nocapture`

use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::time::{Duration, Instant};

use swarmux_application::ports::SessionRepository;
use swarmux_application::{
    GuardService, GuardSettings, HistoryService, JobService, ReviewSettings, SharedPolicy,
};
use swarmux_domain::{JobStatus, Phase, ReviewState, Session, SessionId, Unmatched};
use swarmux_infrastructure::hook::endpoint_for;
use swarmux_infrastructure::{
    ClaudeCli, FsAgentCatalog, GitCli, GuardHook, HookServer, SqliteStore, SystemClock,
};

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=swarmux",
            "-c",
            "user.email=swarmux@example.invalid",
        ])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

#[test]
#[ignore = "runs the real claude CLI"]
fn a_delegating_job_is_reviewed_end_to_end() {
    // A long path: Claude Code treats 8.3 temp paths (AMIRHO~1) as outside
    // the working folder and refuses edits there.
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("swarmux-real-review-{}", std::process::id()));
    let agents = dir.join(".claude").join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(dir.join("app.py"), "def add(a, b):\n    return a + b\n").unwrap();
    std::fs::write(dir.join("util.py"), "def twice(x):\n    return 2 * x\n").unwrap();
    std::fs::write(
        agents.join("scout.md"),
        "---\nname: scout\ndescription: Lists and summarises files. Read-only.\n\
         model: haiku\ntools: Read, Glob, Grep\n---\nYou list files and say what each one defines.\n",
    )
    .unwrap();
    git(&dir, &["init", "-q"]);
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "start"]);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let settings = GuardSettings {
        unmatched: Unmatched::Pass,
        ..GuardSettings::default()
    };
    let policy = SharedPolicy::new(settings.policy());
    let pipe = endpoint_for(std::process::id());
    let server = Rc::new(HookServer::start(rt.handle(), &pipe, policy.clone()).unwrap());
    let bin = std::env::var("SWARMUX_CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
    let runner = Rc::new(
        ClaudeCli::new(bin.into(), rt.handle().clone()).with_guard(GuardHook {
            exe: env!("CARGO_BIN_EXE_swarmux").into(),
            pipe,
            timeout_secs: 60,
        }),
    );
    let store = Rc::new(SqliteStore::open_in_memory().unwrap());
    let clock = Rc::new(SystemClock);
    let mut guard = GuardService::new(policy, server, store.clone(), clock.clone());
    let review = ReviewSettings {
        enabled: true,
        model: "haiku".into(),
    };
    let mut jobs = JobService::new(store.clone(), store.clone(), runner, clock)
        .with_review(Rc::new(GitCli::default()), &review)
        .with_agent_catalog(Rc::new(FsAgentCatalog::new(None)));

    let mut session = Session::new(SessionId::random(), "real", dir.clone(), "haiku", 0).unwrap();
    session.lean = true;
    session.permission_mode = "acceptEdits".into();
    SessionRepository::insert(store.as_ref(), &session).unwrap();
    let job = jobs
        .dispatch(
            session.id,
            "First use the Task tool with subagent_type \"scout\" to list the Python files \
             in this folder and what they define, and wait for its answer. Then use the \
             Edit tool yourself to add this function to the end of app.py: \
             `def health():` returning `{\"ok\": True}`. Reply in one short sentence.",
        )
        .unwrap();

    let start = Instant::now();
    let mut seen = Vec::new();
    loop {
        let report = jobs.pump();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        for d in guard.pump().decisions {
            if let Some(j) = d.job_id {
                jobs.observe_decision(j);
            }
        }
        let p = jobs.progress(job).unwrap();
        let phase = p.phase();
        if seen.last() != Some(&phase) {
            println!(
                "{:>5.1}s  {}",
                start.elapsed().as_secs_f64(),
                phase.as_str()
            );
            seen.push(phase);
        }
        let reviewed = matches!(
            p.review,
            Some(ReviewState::Done { .. } | ReviewState::Failed { .. }) | None
        ) && p.status.is_finished();
        if reviewed || start.elapsed() > Duration::from_secs(420) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let p = jobs.progress(job).unwrap();
    println!(
        "status {:?} · cost ${:.4} · cache read {}",
        p.status,
        p.cost_usd,
        p.cache_read()
    );
    println!("lead tokens {:?}", p.lead_tokens);
    println!(
        "decisions {} · phases {:?}",
        p.decision_marks.len(),
        p.phase_ticks
    );
    for s in &p.subagents {
        println!(
            "subagent {} model {:?} status {:?} calls {} seen {} rate {:?} last {:?}",
            s.kind,
            s.model,
            s.status,
            s.tool_calls,
            s.tokens.seen,
            s.rate(),
            s.last_outcome
        );
    }
    let run = jobs.review_progress(job).unwrap();
    println!(
        "review run: model {} cost ${:.4} tokens {:?}",
        run.model, run.cost_usd, run.lead_tokens
    );
    println!("review: {:?}", p.review);
    println!("command: {:?}", p.command);
    println!("review command: {:?}", run.command);
    let detail = HistoryService::new(store.clone()).detail(job).unwrap();
    println!("stored review: {:?}", detail.review);

    assert_eq!(p.status, JobStatus::Done);
    assert!(!p.subagents.is_empty(), "the lead delegated");
    assert_eq!(p.subagents[0].model.as_deref(), Some("haiku"));
    assert!(seen.contains(&Phase::Delegate));
    // A pass ships; concerns keep the job in audit for you to look at.
    let Some(ReviewState::Done { review, .. }) = &p.review else {
        panic!("no verdict: {:?}", p.review);
    };
    let expected = if review.concerns.is_empty() {
        Phase::Ship
    } else {
        Phase::Audit
    };
    assert_eq!(p.phase(), expected);
    assert!(
        run.lead_tokens.seen > 0 && run.cost_usd > 0.0,
        "review usage is tracked"
    );
    assert!(
        std::fs::read_to_string(dir.join("app.py"))
            .unwrap()
            .contains("def health")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
