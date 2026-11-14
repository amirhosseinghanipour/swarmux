//! A workspace end to end, against real git, SQLite and the fake `claude`:
//! a product folder of three repositories and a plain folder is added, a
//! mission of it runs as one job in a worktree set of the two repositories
//! it names, its checks run in each, and it lands in both, producer first;
//! or, when the second can't take its merge, in neither, and the inbox
//! says so. Run with `cargo test --features fake-claude`.
#![cfg(feature = "fake-claude")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use swarmux_application::ledger::LedgerService;
use swarmux_application::{
    GateKeeper, GateService, GateSource, JobService, LoadedGate, MissionService, ScopeClaims,
    SessionService, WorkspaceService, WorktreeService,
};
use swarmux_domain::gate::{Pin, WorktreeChecks};
use swarmux_domain::mission::{Acceptance, Budget, Constraints, Contract, LandState, MissionState};
use swarmux_domain::{AttentionKind, JobId, Limits, Workspace};
use swarmux_infrastructure::{
    ClaudeCli, FsGateSource, FsWorkspaceScanner, GitMergeQueue, GitWorktrees, ProcessChecker,
    SqliteLedger, SqliteStore, SystemClock,
};

const FAKE: &str = env!("CARGO_BIN_EXE_fake_claude");

fn git(dir: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .expect("git");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    )
}

fn have_git() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn repo(dir: &Path, file: &str) {
    fs::create_dir_all(dir).unwrap();
    assert!(git(dir, &["init", "-q", "-b", "main"]).0);
    fs::write(dir.join(file), "first\n").unwrap();
    fs::write(dir.join("CLAUDE.md"), "repository instructions\n").unwrap();
    assert!(git(dir, &["add", "-A"]).0);
    assert!(git(dir, &["commit", "-q", "-m", "first"]).0);
}

struct Rig {
    dir: tempfile::TempDir,
    _rt: tokio::runtime::Runtime,
    workspace: Workspace,
    jobs: JobService,
    missions: MissionService,
    gate: GateService,
    worktrees: Rc<WorktreeService>,
    slug: String,
}

fn product(dir: &Path) -> PathBuf {
    dir.join("product")
}

/// The product folder, swarmux over it, and a mission naming two of its
/// three repositories, producer first.
///
/// Its checks name a folder per repository, so they run in the task folder
/// and not in a clean copy. Since the verifier checks where checks ran, a
/// mission is only delivered like that when the gate says it may
/// (`[verifier] worktree_checks = "allow"`): this is the rig of the tests
/// about landing, which opt in.
fn rig() -> Option<Rig> {
    rig_with(Some(WorktreeChecks::Allow), None)
}

/// A [`GateSource`] that reads gates as the real one does and sets what a
/// mission does when its checks ran in the worktree, as the `[verifier]`
/// key of a `gates.toml` would.
struct WithWorktreeChecks(FsGateSource, WorktreeChecks);

impl GateSource for WithWorktreeChecks {
    fn load(&self, dir: &Path) -> LoadedGate {
        let mut gate = self.0.load(dir);
        gate.spec.verifier.worktree_checks = self.1;
        gate
    }

    fn pins(&self, root: &Path, globs: &[String]) -> Vec<Pin> {
        self.0.pins(root, globs)
    }
}

/// `worktree_checks` is set on the gate the way a `gates.toml` would; with
/// `None` the gate is the real one, and `root_gates` (if any) is the
/// product root's own `.swarmux/gates.toml`.
fn rig_with(worktree_checks: Option<WorktreeChecks>, root_gates: Option<&str>) -> Option<Rig> {
    if !have_git() {
        eprintln!("git not available; skipping");
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = product(dir.path());
    repo(&root.join("backend"), "schema.sql");
    repo(&root.join("app"), "api.dart");
    repo(&root.join("admin"), "index.ts");
    if let Some(gates) = root_gates {
        fs::create_dir_all(root.join(".swarmux")).unwrap();
        fs::write(root.join(".swarmux/gates.toml"), gates).unwrap();
    }
    fs::create_dir_all(root.join("notes")).unwrap();
    fs::write(root.join("notes").join("ideas.md"), "not a repository\n").unwrap();
    fs::write(
        root.join("CLAUDE.md"),
        "the product: an app and its backend\n",
    )
    .unwrap();

    let store = Rc::new(SqliteStore::open(&dir.path().join("swarmux.db")).unwrap());
    let clock = Rc::new(SystemClock);
    let workspaces = WorkspaceService::new(
        store.clone(),
        Rc::new(FsWorkspaceScanner::default()),
        clock.clone(),
    );
    let workspace = workspaces.add(&root, Some("Product")).unwrap();
    let session = SessionService::new(store.clone(), clock.clone())
        .create_in_workspace("product", &workspace)
        .unwrap();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stream_basic.jsonl");
    let runner = ClaudeCli::new(FAKE.into(), rt.handle().clone()).with_env(vec![
        ("FAKE_CLAUDE_FIXTURE".into(), fixture.display().to_string()),
        (
            "FAKE_CLAUDE_ECHO_FILE".into(),
            dir.path().join("echo.json").display().to_string(),
        ),
    ]);
    let ledger = Rc::new(LedgerService::new(
        Rc::new(SqliteLedger::open(&dir.path().join("swarmux.db")).unwrap()),
        clock.clone(),
    ));
    let worktrees = Rc::new(
        WorktreeService::new(
            Rc::new(GitWorktrees::default()),
            store.clone(),
            store.clone(),
            clock.clone(),
            dir.path().join("wt"),
        )
        .with_ledger(ledger.clone()),
    );
    let jobs = JobService::new(store.clone(), store.clone(), Rc::new(runner), clock.clone())
        .with_limits(Limits {
            max_concurrent: 2,
            min_free_ram: 0,
        })
        .with_worktrees(worktrees.clone())
        .with_workspaces(store.clone());
    let source: Arc<dyn GateSource> = match worktree_checks {
        Some(w) => Arc::new(WithWorktreeChecks(FsGateSource::new(), w)),
        None => Arc::new(FsGateSource::new()),
    };
    let (keeper, records) = GateKeeper::new(source, Arc::new(ProcessChecker::new()));
    let keeper = Arc::new(keeper);
    let gate = GateService::new(records, ledger.clone(), store.clone(), clock.clone());
    let mut missions = MissionService::new(store.clone(), clock.clone())
        .unwrap()
        .with_gates(Rc::new(keeper))
        .with_ledger(ledger)
        .with_workspaces(store.clone())
        .with_graph(
            Rc::new(GitMergeQueue::under(dir.path().join("merge"))),
            worktrees.clone(),
            ScopeClaims::new(),
        );
    let contract = Contract {
        title: "Order endpoint".into(),
        goal: "add the orders table and call it from the app".into(),
        repos: vec!["backend".into(), "app".into()],
        acceptance: Acceptance {
            // Inside a repository, not in the task folder above them.
            checks: vec![
                "backend: git rev-parse --is-inside-work-tree".into(),
                "app: git rev-parse --is-inside-work-tree".into(),
            ],
            human_signoff: false,
        },
        constraints: Constraints {
            paths_rw: vec!["backend/**".into(), "app/**".into()],
            protected: Vec::new(),
        },
        budget: Budget {
            usd: Some(1.0),
            ..Budget::default()
        },
        due: None,
        clarifications: Vec::new(),
    };
    let slug = missions
        .create_in(
            contract,
            &root.display().to_string(),
            Some(session.id),
            Some(workspace.id),
        )
        .unwrap();
    Some(Rig {
        dir,
        _rt: rt,
        workspace,
        jobs,
        missions,
        gate,
        worktrees,
        slug,
    })
}

impl Rig {
    /// Pumps everything until the mission settles.
    fn settle(&mut self) -> MissionState {
        let end = Instant::now() + Duration::from_secs(60);
        loop {
            self.jobs.pump();
            self.gate.pump();
            let facts = self.jobs.facts();
            let report = self.missions.pump(&facts, self.gate.states());
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            let state = self.missions.get(&self.slug).unwrap().state;
            if state.is_final() || Instant::now() > end {
                return state;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// What the agent would have done: a change in each repository of
    /// its set.
    fn work(&self, job: JobId) {
        for r in self.worktrees.set_for(job).unwrap() {
            let file = match r.repo_name() {
                Some("backend") => "schema.sql",
                _ => "api.dart",
            };
            fs::write(r.path.join(file), "first\nthe orders change\n").unwrap();
        }
    }

    fn echo(&self) -> serde_json::Value {
        let text = fs::read_to_string(self.dir.path().join("echo.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn landed(&self, repo: &str, job: JobId) -> bool {
        let branch = format!("swarmux/product/{}", job.0);
        let into = format!("swarmux/{}/integration", self.slug);
        git(
            &product(self.dir.path()).join(repo),
            &["merge-base", "--is-ancestor", &branch, &into],
        )
        .0
    }
}

#[test]
fn a_workspace_mission_runs_in_a_set_and_lands_producer_first() {
    let Some(mut r) = rig() else { return };
    assert_eq!(
        r.workspace.repo_names(),
        ["admin", "app", "backend"],
        "the plain folder is no repository"
    );
    let job = r.missions.launch(&r.slug, &mut r.jobs).unwrap();
    let set = r.worktrees.set_for(job).unwrap();
    let names: Vec<_> = set.iter().filter_map(|w| w.repo_name()).collect();
    assert_eq!(names, ["backend", "app"], "admin is not touched");
    let task = set[0].member.as_ref().unwrap().task_dir.clone();
    assert_eq!(set[0].path, task.join("backend"));
    assert_eq!(set[1].path, task.join("app"));
    assert!(
        task.join("CLAUDE.md").is_file(),
        "the root's own instructions"
    );
    assert!(
        !task.join("notes").exists(),
        "folders outside repositories stay"
    );
    assert!(!task.join("admin").exists());
    r.work(job);

    assert_eq!(
        r.settle(),
        MissionState::Delivered,
        "{:?}",
        r.missions.get(&r.slug)
    );

    // claude ran from the task folder with both repositories added and
    // their instructions loaded.
    let echo = r.echo();
    let cwd = PathBuf::from(echo["cwd"].as_str().unwrap());
    assert_eq!(
        fs::canonicalize(&cwd).unwrap(),
        fs::canonicalize(&task).unwrap()
    );
    let args: Vec<String> = echo["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().to_string())
        .collect();
    let at = args.iter().position(|a| a == "--add-dir").unwrap();
    assert_eq!(
        args[at + 1..at + 3],
        [
            set[0].path.display().to_string(),
            set[1].path.display().to_string()
        ]
    );
    assert_eq!(echo["added_dirs_claude_md"], "1");

    // The gate ran each repository's check in that repository.
    let state = r.gate.states().get(&job).unwrap();
    let checks: Vec<(&str, bool)> = state
        .checks
        .iter()
        .map(|(name, ok, _)| (name.as_str(), *ok))
        .collect();
    assert_eq!(checks.len(), 2, "{checks:?}");
    assert!(checks.iter().all(|(_, ok)| *ok), "{checks:?}");
    assert!(checks[0].0.starts_with("backend: "), "{checks:?}");

    // Both landed, the producer first, each in its own repository.
    let landing = r.missions.get(&r.slug).unwrap().landing.clone().unwrap();
    let order: Vec<(&str, LandState)> = landing
        .repos
        .iter()
        .map(|l| (l.repo.as_str(), l.state))
        .collect();
    assert_eq!(
        order,
        [("backend", LandState::Landed), ("app", LandState::Landed)]
    );
    assert!(r.landed("backend", job));
    assert!(r.landed("app", job));
    assert!(
        !git(
            &product(r.dir.path()).join("admin"),
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "refs/heads/swarmux/order-endpoint/integration"
            ]
        )
        .0
    );
    // The user's own branches are left alone.
    let (_, main) = git(
        &product(r.dir.path()).join("backend"),
        &["show", "main:schema.sql"],
    );
    assert_eq!(main, "first");
}

#[test]
fn a_workspace_mission_with_in_place_checks_is_escalated_by_default_and_lands_nothing() {
    // The default (`worktree_checks = "escalate"`): its checks ran in the
    // task folder, not in a clean copy, so the gate cannot pass it and
    // nothing is landed until a person decides.
    let Some(mut r) = rig_with(None, None) else {
        return;
    };
    let job = r.missions.launch(&r.slug, &mut r.jobs).unwrap();
    r.work(job);
    assert_eq!(
        r.settle(),
        MissionState::Escalated,
        "{:?}",
        r.missions.get(&r.slug)
    );
    assert!(!r.landed("backend", job));
    assert!(!r.landed("app", job));
    let state = r.gate.state(job).expect("a verdict");
    assert_eq!(
        state.notes.ran_in,
        Some(swarmux_domain::gate::RanIn::Worktree)
    );
    assert!(
        state
            .notes
            .incomplete
            .iter()
            .any(|s| s.contains("ran in the worktree")),
        "{:?}",
        state.notes.incomplete
    );
}

#[test]
fn the_product_roots_own_gates_toml_is_how_a_workspace_mission_opts_in_and_lands() {
    // No test double: the root's `.swarmux/gates.toml` is copied into the
    // task folder, where the gate reads it.
    let gates = "[verifier]\nworktree_checks = \"allow\"\n";
    let Some(mut r) = rig_with(None, Some(gates)) else {
        return;
    };
    let job = r.missions.launch(&r.slug, &mut r.jobs).unwrap();
    let set = r.worktrees.set_for(job).unwrap();
    let task = set[0].member.as_ref().unwrap().task_dir.clone();
    assert!(task.join(".swarmux/gates.toml").is_file());
    r.work(job);
    assert_eq!(
        r.settle(),
        MissionState::Delivered,
        "{:?}",
        r.missions.get(&r.slug)
    );
    assert!(r.landed("backend", job) && r.landed("app", job));
    assert_eq!(
        r.gate.state(job).unwrap().notes.ran_in,
        Some(swarmux_domain::gate::RanIn::Worktree),
        "still recorded as run in the worktree"
    );
}

#[test]
fn a_repo_that_cannot_land_leaves_the_first_unlanded_and_asks() {
    let Some(mut r) = rig() else { return };
    let job = r.missions.launch(&r.slug, &mut r.jobs).unwrap();
    r.work(job);
    // Meanwhile app's own branch changed the same line: its merge conflicts.
    let app = product(r.dir.path()).join("app");
    fs::write(app.join("api.dart"), "first\nsomeone else's change\n").unwrap();
    assert!(git(&app, &["commit", "-q", "-am", "elsewhere"]).0);

    assert_eq!(r.settle(), MissionState::Escalated);
    let m = r.missions.get(&r.slug).unwrap().clone();
    let landing = m.landing.clone().unwrap();
    let states: Vec<LandState> = landing.repos.iter().map(|l| l.state).collect();
    assert_eq!(states, [LandState::Undone, LandState::Failed]);
    assert!(landing.repos[1].note.contains("api.dart"), "{landing:?}");
    assert!(!r.landed("backend", job), "the producer was put back");
    assert!(!r.landed("app", job));
    let items = r.missions.plan_items(i64::MAX / 2);
    let item = items
        .iter()
        .find(|i| i.kind == AttentionKind::Merge)
        .expect("an inbox item");
    assert!(
        item.detail.contains("app could not land"),
        "{}",
        item.detail
    );
}
