//! Job worktrees against real git and SQLite: eight at once, ports that
//! never collide, resuming after a quit, checkpoints and reclaiming.

use std::cell::Cell;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use swarmux_application::ledger::{Ledger, LedgerService};
use swarmux_application::ports::{
    AgentRunner, Clock, JobRepository, RunRequest, RunSink, RunnerMsg, SessionRepository,
};
use swarmux_application::{
    AddWorktree, CheckpointService, HandoffBoard, JobService, Notes, WorktreeService,
    WorktreeSettings, Worktrees,
};
use swarmux_domain::ledger::LedgerKind;
use swarmux_domain::{
    AgentEvent, CheckpointReason, JobId, JobStatus, Limits, Millis, PortPlan, Session, SessionId,
};
use swarmux_infrastructure::{GitWorktrees, SqliteLedger, SqliteStore};

const HOUR: Millis = 3_600_000;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn have_git() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// A clock that can be moved forward.
struct Skew(Cell<Millis>);

impl Clock for Skew {
    fn now(&self) -> Millis {
        let real = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as Millis);
        real + self.0.get()
    }
}

struct Env {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    db: PathBuf,
    root: PathBuf,
    clock: Rc<Skew>,
}

fn env() -> Option<Env> {
    if !have_git() {
        eprintln!("git not available; skipping");
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "demo\n").unwrap();
    std::fs::write(repo.join(".gitignore"), ".env\n").unwrap();
    std::fs::write(repo.join(".env"), "TOKEN=abc\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    let db = dir.path().join("swarmux.db");
    let root = dir.path().join("wt");
    Some(Env {
        _dir: dir,
        repo,
        db,
        root,
        clock: Rc::new(Skew(Cell::new(0))),
    })
}

/// Ports far from anything usually listening.
fn settings() -> WorktreeSettings {
    WorktreeSettings {
        ports: PortPlan {
            base: 47_000,
            block: 20,
            slots: 64,
        },
        ..WorktreeSettings::default()
    }
}

struct App {
    store: Rc<SqliteStore>,
    ledger: Rc<SqliteLedger>,
    worktrees: Rc<WorktreeService>,
    checkpoints: Rc<CheckpointService>,
    board: Arc<HandoffBoard>,
}

/// What a process of the app has: the services over the shared database.
fn app(env: &Env) -> App {
    let store = Rc::new(SqliteStore::open(&env.db).unwrap());
    let ledger = Rc::new(SqliteLedger::open(&env.db).unwrap());
    let service = Rc::new(LedgerService::new(ledger.clone(), env.clock.clone()));
    let board = HandoffBoard::new();
    let worktrees = Rc::new(
        WorktreeService::new(
            Rc::new(GitWorktrees::default()),
            store.clone(),
            store.clone(),
            env.clock.clone(),
            env.root.clone(),
        )
        .with_settings(&settings())
        .with_ledger(service.clone())
        .with_handoffs(board.clone()),
    );
    let checkpoints = Rc::new(
        CheckpointService::new(worktrees.clone(), store.clone(), service, env.clock.clone())
            .with_handoffs(board.clone()),
    );
    App {
        store,
        ledger,
        worktrees,
        checkpoints,
        board,
    }
}

fn jobs(app: &App, env: &Env, runner: Rc<dyn AgentRunner>) -> JobService {
    JobService::new(
        app.store.clone(),
        app.store.clone(),
        runner,
        env.clock.clone(),
    )
    .with_limits(Limits {
        max_concurrent: 8,
        min_free_ram: 0,
    })
    .with_worktrees(app.worktrees.clone())
}

fn session(app: &App, env: &Env, name: &str) -> Session {
    let s = Session::new(SessionId::random(), name, env.repo.clone(), "sonnet", 0).unwrap();
    SessionRepository::insert(&*app.store, &s).unwrap();
    s
}

fn done_event() -> AgentEvent {
    AgentEvent::Result {
        is_error: false,
        subtype: "success".into(),
        cost_usd: 0.0,
        duration_ms: 1,
        num_turns: 1,
        tokens_in: 1,
        tokens_out: 1,
        cache_read: 0,
        context_window: None,
        subagents_spawned: 0,
        text: "done".into(),
    }
}

fn init_event(id: &str) -> AgentEvent {
    AgentEvent::Init {
        session_id: id.into(),
        model: "claude-sonnet".into(),
        cwd: String::new(),
        permission_mode: "default".into(),
    }
}

/// What a scripted `claude` saw when it started.
#[derive(Clone, Debug)]
struct Seen {
    request: RunRequest,
    /// Step files already in its folder.
    steps_before: usize,
}

/// Stands in for `claude`: does `total` steps in its folder, one file per
/// step, and can be told to stop after some of them (a quit).
struct Script {
    total: usize,
    stop_after: AtomicUsize,
    quit: Arc<AtomicBool>,
    seen: Mutex<Vec<Seen>>,
    /// Ports held open while the run lasts, by job.
    hold_ports: bool,
    held: Mutex<Vec<TcpListener>>,
    bind_errors: AtomicUsize,
}

impl Script {
    fn new(total: usize) -> Arc<Script> {
        Arc::new(Script {
            total,
            stop_after: AtomicUsize::new(usize::MAX),
            quit: Arc::new(AtomicBool::new(false)),
            seen: Mutex::new(Vec::new()),
            hold_ports: false,
            held: Mutex::new(Vec::new()),
            bind_errors: AtomicUsize::new(0),
        })
    }

    fn holding_ports(total: usize) -> Arc<Script> {
        let mut s = Script::new(total);
        Arc::get_mut(&mut s).unwrap().hold_ports = true;
        s
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn step_files(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("step-"))
                .count()
        })
        .unwrap_or(0)
}

struct ScriptRunner(Arc<Script>);

impl AgentRunner for ScriptRunner {
    fn start(&self, request: RunRequest, sink: RunSink) {
        let script = self.0.clone();
        let steps_before = step_files(&request.cwd);
        script.seen.lock().unwrap().push(Seen {
            request: request.clone(),
            steps_before,
        });
        if script.hold_ports {
            // Take the job's first and last port, as a dev server would.
            let base: u16 = request
                .env
                .iter()
                .find(|(k, _)| k == "SWARMUX_PORT_BASE")
                .and_then(|(_, v)| v.parse().ok())
                .expect("a port base");
            let count: u16 = request
                .env
                .iter()
                .find(|(k, _)| k == "SWARMUX_PORT_COUNT")
                .and_then(|(_, v)| v.parse().ok())
                .expect("a port count");
            for port in [base, base + count - 1] {
                match TcpListener::bind(("127.0.0.1", port)) {
                    Ok(l) => script.held.lock().unwrap().push(l),
                    Err(_) => {
                        script.bind_errors.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
        }
        thread::spawn(move || {
            sink(RunnerMsg::Event(init_event(
                &request.session_id.to_string(),
            )));
            let mut n = steps_before;
            while n < script.total {
                if script.quit.load(Ordering::SeqCst)
                    || n >= script.stop_after.load(Ordering::SeqCst)
                {
                    // The app quit before the next step finished.
                    return;
                }
                n += 1;
                std::fs::write(request.cwd.join(format!("step-{n}.txt")), format!("{n}\n")).ok();
                thread::sleep(Duration::from_millis(5));
            }
            sink(RunnerMsg::Event(done_event()));
            sink(RunnerMsg::Exited {
                code: Some(0),
                stderr_tail: String::new(),
            });
        });
    }

    fn cancel(&self, _: JobId) {}
}

fn pump_until(svc: &mut JobService, what: &str, mut ready: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(60);
    while !ready() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        let report = svc.pump();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn eight_jobs_at_once_get_eight_worktrees_and_no_port_collisions() {
    let Some(env) = env() else { return };
    let app = app(&env);
    let script = Script::holding_ports(1);
    let mut svc = jobs(&app, &env, Rc::new(ScriptRunner(script.clone())));
    let sessions: Vec<Session> = (0..8)
        .map(|n| session(&app, &env, &format!("feature {n}")))
        .collect();
    let ids: Vec<JobId> = sessions
        .iter()
        .map(|s| svc.dispatch(s.id, "build it").unwrap())
        .collect();

    // All eight started together (a ninth would have had to wait).
    assert_eq!(script.seen().len(), 8);
    assert_eq!(
        script.bind_errors.load(Ordering::SeqCst),
        0,
        "every job could bind both ends of its port block"
    );

    let seen = script.seen();
    let mut bases: Vec<u16> = seen
        .iter()
        .map(|s| {
            s.request
                .env
                .iter()
                .find(|(k, _)| k == "SWARMUX_PORT_BASE")
                .unwrap()
                .1
                .parse()
                .unwrap()
        })
        .collect();
    bases.sort_unstable();
    bases.dedup();
    assert_eq!(bases.len(), 8, "{bases:?}");
    let mut cwds: Vec<_> = seen.iter().map(|s| s.request.cwd.clone()).collect();
    cwds.sort();
    cwds.dedup();
    assert_eq!(cwds.len(), 8);

    for (job, s) in ids.iter().zip(&seen) {
        let cwd = &s.request.cwd;
        // A real checkout on its own branch, with the ignored .env copied in.
        assert!(cwd.join("README.md").is_file());
        assert_eq!(
            std::fs::read_to_string(cwd.join(".env")).unwrap(),
            "TOKEN=abc\n"
        );
        let record = app.worktrees.record_for(*job).unwrap().unwrap();
        assert_eq!(git(cwd, &["branch", "--show-current"]), record.branch);
        assert!(
            record.branch.starts_with("swarmux/feature-"),
            "{}",
            record.branch
        );
    }
    let listed = git(&env.repo, &["worktree", "list", "--porcelain"]);
    assert_eq!(listed.matches("worktree ").count(), 9, "{listed}");

    let end = Instant::now() + Duration::from_secs(30);
    while ids
        .iter()
        .any(|j| !app.store.status(*j).unwrap().is_finished())
        && Instant::now() < end
    {
        svc.pump();
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        ids.iter()
            .all(|j| app.store.status(*j).unwrap() == JobStatus::Done)
    );
    // Each worktree is recorded in the ledger.
    let created = app
        .ledger
        .range(1, 10_000)
        .unwrap()
        .iter()
        .filter(|e| e.draft.kind == LedgerKind::Worktree)
        .count();
    assert_eq!(created, 8);
}

#[test]
fn parallel_git_adds_do_not_trip_over_each_other() {
    let Some(env) = env() else { return };
    let repo = env.repo.clone();
    let handles: Vec<_> = (0..8)
        .map(|n| {
            let repo = repo.clone();
            let path = env.root.join(format!("p{n}"));
            thread::spawn(move || {
                GitWorktrees::default().add(&AddWorktree {
                    repo,
                    path,
                    branch: format!("swarmux/par/{n}"),
                    copy: vec![".env".into()],
                })
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap().expect("add");
    }
    let listed = git(&env.repo, &["worktree", "list", "--porcelain"]);
    assert_eq!(listed.matches("worktree ").count(), 9);
}

/// Runs the first half of the resume story: three steps done, the fourth in
/// flight when the app quits. Returns the job and the worktree folder.
fn run_and_quit(env: &Env, with_checkpoint: bool) -> (JobId, SessionId, PathBuf) {
    let app = app(env);
    let script = Script::new(6);
    script.stop_after.store(3, Ordering::SeqCst);
    let mut svc = jobs(&app, env, Rc::new(ScriptRunner(script.clone())));
    let s = session(&app, env, "long task");
    let job = svc.dispatch(s.id, "do six steps").unwrap();
    let cwd = script.seen()[0].request.cwd.clone();
    pump_until(&mut svc, "three steps", || step_files(&cwd) == 3);
    if with_checkpoint {
        // The done-gate passed after step 3.
        app.checkpoints
            .take(
                job,
                &s.id.to_string(),
                CheckpointReason::GatePass,
                &Notes::default(),
            )
            .unwrap()
            .expect("checkpoint");
    }
    // The step in flight when the app quits leaves nothing behind.
    script.quit.store(true, Ordering::SeqCst);
    (job, s.id, cwd)
}

#[test]
fn resume_after_quit_loses_at_most_one_step() {
    let Some(env) = env() else { return };
    let (job, session_id, cwd) = run_and_quit(&env, true);

    // A new run of the app over the same database.
    let app = app(&env);
    let script = Script::new(6);
    let mut svc = jobs(&app, &env, Rc::new(ScriptRunner(script.clone())));
    assert_eq!(svc.recover().unwrap(), 1, "the quit job is interrupted");
    let resumed = svc.resume(job).unwrap();

    let seen = script.seen();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].request.resume, "continues the Claude session");
    assert_eq!(seen[0].request.session_id, session_id);
    assert_eq!(seen[0].request.cwd, cwd, "same worktree");
    // Three steps were done before the quit and are still there; only the
    // step in flight (the fourth) has to be redone.
    assert_eq!(seen[0].steps_before, 3);
    let lost = 4 - seen[0].steps_before;
    assert!(lost <= 1, "lost {lost} steps");

    pump_until(&mut svc, "the resumed job", || {
        app.store.status(resumed).unwrap().is_finished()
    });
    assert_eq!(app.store.status(resumed).unwrap(), JobStatus::Done);
    assert_eq!(step_files(&cwd), 6);
    // Checkpoints followed the job.
    assert_eq!(app.checkpoints.list(resumed).unwrap().len(), 1);
    assert!(app.checkpoints.list(job).unwrap().is_empty());
    // The Claude session of the checkpoint is the one that was resumed.
    let c = app.checkpoints.latest(resumed).unwrap().unwrap();
    assert_eq!(c.session_id, session_id.to_string());
    // The handoff reaches the resumed session.
    assert!(app.board.get(resumed).unwrap().contains("do six steps"));
}

#[test]
fn a_checkpoint_brings_the_work_back_when_the_folder_is_lost() {
    let Some(env) = env() else { return };
    let (job, _, cwd) = run_and_quit(&env, true);
    // The folder is gone (disk cleanup, a crash): only git remembers it.
    let record = {
        let app = app(&env);
        app.worktrees.record_for(job).unwrap().unwrap()
    };
    let removed = Command::new("git")
        .arg("-C")
        .arg(&env.repo)
        .args(["worktree", "remove", "--force"])
        .arg(&cwd)
        .output()
        .unwrap();
    assert!(removed.status.success());
    assert!(!cwd.exists());

    let app = app(&env);
    let script = Script::new(6);
    let mut svc = jobs(&app, &env, Rc::new(ScriptRunner(script.clone())));
    svc.recover().unwrap();
    let resumed = svc.resume(job).unwrap();
    let seen = script.seen();
    assert_eq!(seen[0].request.cwd, record.path);
    // Restored from the branch the checkpoint committed to.
    assert_eq!(seen[0].steps_before, 3);
    assert_eq!(
        std::fs::read_to_string(cwd.join(".env")).unwrap(),
        "TOKEN=abc\n"
    );
    pump_until(&mut svc, "the resumed job", || {
        app.store.status(resumed).unwrap().is_finished()
    });
    assert_eq!(step_files(&cwd), 6);

    let c = app.checkpoints.list(resumed).unwrap();
    let shown = git(&env.repo, &["show", &format!("{}:step-3.txt", c[0].commit)]);
    assert_eq!(shown.trim(), "3");
}

#[test]
fn checkpoints_commit_the_work_and_record_it_in_the_ledger() {
    let Some(env) = env() else { return };
    let (job, _, cwd) = run_and_quit(&env, true);
    let app = app(&env);
    let c = app.checkpoints.latest(job).unwrap().unwrap();
    assert_eq!(c.worktree, cwd);
    let files = git(&env.repo, &["ls-tree", "-r", "--name-only", &c.commit]);
    assert!(files.contains("step-3.txt"), "{files}");
    assert!(!files.contains(".env"));
    let seq = c.ledger_seq.expect("ledger seq");
    let entry = app.ledger.range(seq, 1).unwrap().remove(0);
    assert_eq!(entry.draft.kind, LedgerKind::Checkpoint);
    assert!(entry.draft.payload_json.contains(&c.commit));
    assert_eq!(
        swarmux_application::ledger::verify_all(&*app.ledger)
            .unwrap()
            .broken,
        None
    );
}

#[test]
fn gc_reclaims_stale_worktrees_but_keeps_their_branches_and_work() {
    let Some(env) = env() else { return };
    let app = app(&env);
    let script = Script::new(2);
    let mut svc = jobs(&app, &env, Rc::new(ScriptRunner(script.clone())));
    let s = session(&app, &env, "gc me");
    let job = svc.dispatch(s.id, "two steps").unwrap();
    pump_until(&mut svc, "the job", || {
        app.store.status(job).unwrap().is_finished()
    });
    let record = app.worktrees.record_for(job).unwrap().unwrap();
    // Work the agent never committed.
    std::fs::write(record.path.join("unsaved.txt"), "keep me").unwrap();

    // Not stale yet.
    let report = app.worktrees.gc(false).unwrap();
    assert!(report.reclaimed.is_empty());
    assert!(record.path.exists());

    // 25 hours later: reclaimed within the day.
    env.clock.0.set(25 * HOUR);
    let dry = app.worktrees.gc(true).unwrap();
    assert_eq!(dry.reclaimed.len(), 1);
    assert!(record.path.exists(), "a dry run changes nothing");

    let report = app.worktrees.gc(false).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.reclaimed.len(), 1);
    assert!(!record.path.exists());
    assert!(app.worktrees.record_for(job).unwrap().is_none());
    // The branch still has the files, unsaved ones included.
    let files = git(&env.repo, &["ls-tree", "-r", "--name-only", &record.branch]);
    assert!(files.contains("unsaved.txt"), "{files}");
    assert!(files.contains("step-2.txt"), "{files}");
    // The ledger says so.
    let reclaimed = app.ledger.range(1, 10_000).unwrap().iter().any(|e| {
        e.draft.kind == LedgerKind::Worktree && e.draft.payload_json.contains("reclaimed")
    });
    assert!(reclaimed);
    // And the port block is free again.
    let next = session(&app, &env, "next");
    let job2 = svc.dispatch(next.id, "again").unwrap();
    assert_eq!(
        app.worktrees.record_for(job2).unwrap().unwrap().slot,
        record.slot
    );
}

#[test]
fn worktree_mode_follows_the_session_and_the_folder() {
    let Some(env) = env() else { return };
    let app = app(&env);
    let script = Script::new(1);
    let mut svc = jobs(&app, &env, Rc::new(ScriptRunner(script.clone())));

    let off = session(&app, &env, "no worktree");
    app.store.set_worktree(off.id, Some(false)).unwrap();
    assert_eq!(
        SessionRepository::get(&*app.store, off.id)
            .unwrap()
            .unwrap()
            .worktree,
        Some(false)
    );
    svc.dispatch(off.id, "x").unwrap();
    assert_eq!(script.seen()[0].request.cwd, env.repo);

    // Outside a repository the default is off.
    let plain = tempfile::tempdir().unwrap();
    let outside = Session::new(
        SessionId::random(),
        "plain",
        plain.path().into(),
        "sonnet",
        0,
    )
    .unwrap();
    SessionRepository::insert(&*app.store, &outside).unwrap();
    assert!(!app.worktrees.enabled_for(&outside));
    svc.dispatch(outside.id, "x").unwrap();
    assert_eq!(script.seen()[1].request.cwd, plain.path());
    assert!(script.seen()[1].request.env.is_empty());

    // Back to auto inside a repository: on.
    app.store.set_worktree(off.id, None).unwrap();
    let again = SessionRepository::get(&*app.store, off.id)
        .unwrap()
        .unwrap();
    assert_eq!(again.worktree, None);
    assert!(app.worktrees.enabled_for(&again));
}
