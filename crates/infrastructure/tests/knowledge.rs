//! Knowledge end to end, against SQLite, the real hook socket and the fake
//! `claude` making the hook calls Claude Code would: a workspace job that
//! reads a file a rule is scoped to gets the rule in its context, and its
//! edit that breaks a guard rule in `ask` waits for the owner as a P1
//! approval; the same rule in shadow only logs. Run with
//! `cargo test --features fake-claude`.
#![cfg(feature = "fake-claude")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use swarmux_application::ledger::{Ledger, LedgerService};
use swarmux_application::{
    AttentionService, GuardService, JobService, KnowledgeService, SessionService, SharedPolicy,
    WorkspaceService,
};
use swarmux_domain::knowledge::{Action, Draft, Enforcer, GuardRule, Home, ItemId, Kind, Status};
use swarmux_domain::ledger::LedgerKind;
use swarmux_domain::{AttentionKind, GuardPolicy, JobId, Pattern, Priority, Session, Unmatched};
use swarmux_infrastructure::hook::{Responders, endpoint_for};
use swarmux_infrastructure::{
    ClaudeCli, FsAnchorReader, FsWorkspaceScanner, GuardHook, HookServer, SqliteLedger,
    SqliteStore, SystemClock,
};

const FAKE: &str = env!("CARGO_BIN_EXE_fake_claude");

fn git(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}

fn have_git() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

struct Rig {
    dir: tempfile::TempDir,
    _rt: tokio::runtime::Runtime,
    _server: Rc<HookServer>,
    jobs: JobService,
    guard: GuardService,
    knowledge: Rc<KnowledgeService>,
    ledger: Rc<SqliteLedger>,
    session: Session,
}

/// A product folder with a backend repository, swarmux over it with
/// knowledge on, and the fake `claude` making the calls in `hooks.jsonl`.
fn rig() -> Option<Rig> {
    if !have_git() {
        eprintln!("git not available; skipping");
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("product");
    let backend = root.join("backend");
    fs::create_dir_all(backend.join("supabase/migrations")).unwrap();
    fs::write(backend.join("supabase/config.toml"), "project_id = \"p\"\n").unwrap();
    fs::write(
        backend.join("supabase/migrations/001_users.sql"),
        "create table users (id uuid primary key);\n",
    )
    .unwrap();
    assert!(git(&backend, &["init", "-q", "-b", "main"]));
    assert!(git(&backend, &["add", "-A"]));
    assert!(git(&backend, &["commit", "-q", "-m", "first"]));

    let db = dir.path().join("swarmux.db");
    let store = Rc::new(SqliteStore::open(&db).unwrap());
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
    let raw_ledger = Rc::new(SqliteLedger::open(&db).unwrap());
    let ledger = Rc::new(LedgerService::new(raw_ledger.clone(), clock.clone()));
    let knowledge = Rc::new(
        KnowledgeService::new(store.clone(), clock.clone())
            .unwrap()
            .with_ledger(ledger)
            .with_reader(Rc::new(FsAnchorReader))
            .with_workspaces(store.clone()),
    );
    let home = Home::Workspace(workspace.id);
    knowledge
        .save_map(
            &home,
            &["Product is a marketplace with a Supabase backend.".into()],
        )
        .unwrap();
    let mut scoped = Draft::new(home.clone(), Kind::Rule, "Migrations are append-only.");
    scoped.scope.paths = vec!["backend/supabase/migrations/**".into()];
    knowledge.add(scoped).unwrap();
    let mut guard_rule = Draft::new(home, Kind::Rule, "Anonymous users are full users.");
    guard_rule.why = "agents once 'fixed' that and broke the backend".into();
    guard_rule.enforcer = Some(Enforcer::Guard(GuardRule {
        tools: vec!["Edit".into()],
        paths: vec!["backend/supabase/migrations/**".into()],
        pattern: Some("is_anonymous".into()),
        action: Action::Ask,
        message: "show the owner first".into(),
    }));
    guard_rule.status = Some(Status::Enforced);
    knowledge.add(guard_rule).unwrap();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let hub = knowledge.hub();
    let policy = SharedPolicy::new(GuardPolicy::new(
        vec![
            Pattern::parse("Edit").unwrap(),
            Pattern::parse("Read").unwrap(),
        ],
        Vec::new(),
        Unmatched::Pass,
    ))
    .with_rules(hub.clone());
    static NEXT: AtomicU32 = AtomicU32::new(1);
    let pipe = endpoint_for(std::process::id() ^ (NEXT.fetch_add(1, Ordering::Relaxed) << 20));
    let server = Rc::new(
        HookServer::start_with(
            rt.handle(),
            &pipe,
            policy.clone(),
            Responders {
                session_start: hub.clone(),
                tool_context: hub,
                ..Responders::default()
            },
        )
        .unwrap(),
    );
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stream_basic.jsonl");
    let runner = ClaudeCli::new(FAKE.into(), rt.handle().clone())
        .with_guard(GuardHook {
            exe: PathBuf::from(FAKE),
            pipe,
            timeout_secs: 60,
        })
        .with_env(vec![
            ("FAKE_CLAUDE_FIXTURE".into(), fixture.display().to_string()),
            (
                "FAKE_CLAUDE_HOOKS".into(),
                dir.path().join("hooks.jsonl").display().to_string(),
            ),
            (
                "FAKE_CLAUDE_HOOKS_OUT".into(),
                dir.path().join("hooks.out").display().to_string(),
            ),
        ]);
    let jobs = JobService::new(store.clone(), store.clone(), Rc::new(runner), clock.clone())
        .with_workspaces(store.clone())
        .with_starts(knowledge.clone());
    let guard = GuardService::new(policy, server.clone(), store.clone(), clock.clone())
        .with_timeout_secs(60);
    Some(Rig {
        dir,
        _rt: rt,
        _server: server,
        jobs,
        guard,
        knowledge,
        ledger: raw_ledger,
        session,
    })
}

/// The calls the agent makes: a session start, a read of a migration and
/// a new migration that conditions on anonymous users.
const CALLS: &str = r#"{"event":"SessionStart","input":{"session_id":"s","hook_event_name":"SessionStart","source":"startup"}}
{"event":"PreToolUse","input":{"session_id":"s","hook_event_name":"PreToolUse","tool_name":"Read","tool_input":{"file_path":"{cwd}/backend/supabase/migrations/001_users.sql"}}}
{"event":"PreToolUse","input":{"session_id":"s","hook_event_name":"PreToolUse","tool_name":"Write","tool_input":{"file_path":"{cwd}/backend/supabase/migrations/002_guests.sql","content":"create policy no_guests on orders using (not (auth.jwt()->>'is_anonymous')::boolean);"}}}
"#;

impl Rig {
    /// Runs one job to its end, answering the approvals it raises with
    /// `answer`; returns what each hook call printed.
    fn run(
        &mut self,
        answer: &mut dyn FnMut(&mut GuardService, &[swarmux_domain::AttentionItem]),
    ) -> Vec<String> {
        let out = self.dir.path().join("hooks.out");
        let _ = fs::remove_file(&out);
        fs::write(self.dir.path().join("hooks.jsonl"), CALLS).unwrap();
        let job = self
            .jobs
            .dispatch(self.session.id, "add guest orders")
            .unwrap();
        let end = Instant::now() + Duration::from_secs(60);
        let mut attention = AttentionService::new();
        loop {
            self.jobs.pump();
            self.guard.pump();
            assert!(self.knowledge.pump().is_empty());
            let held = self.guard.approvals();
            if !held.is_empty() {
                let items = attention.collect_from(&held, &self.jobs.facts(), SystemClock.now_ms());
                answer(&mut self.guard, &items);
            }
            let done = self
                .jobs
                .progress(job)
                .is_some_and(|p| p.status.is_finished());
            if done || Instant::now() > end {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            self.jobs.progress(job).unwrap().status.is_finished(),
            "job {job} did not end"
        );
        self.guard.pump();
        assert!(self.knowledge.pump().is_empty());
        fs::read_to_string(out)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn knowledge_entries(&self) -> Vec<(Option<JobId>, String)> {
        let head = self.ledger.head().unwrap().unwrap().seq;
        self.ledger
            .range(1, head as usize)
            .unwrap()
            .into_iter()
            .filter(|e| e.draft.kind == LedgerKind::Knowledge)
            .map(|e| (e.draft.job, e.draft.payload_json))
            .collect()
    }
}

trait NowMs {
    fn now_ms(&self) -> i64;
}

impl NowMs for SystemClock {
    fn now_ms(&self) -> i64 {
        swarmux_application::ports::Clock::now(self)
    }
}

#[test]
fn a_workspace_job_gets_triggered_knowledge_and_a_rule_that_asks_holds_its_edit() {
    let Some(mut rig) = rig() else { return };
    let mut asked = Vec::new();
    let out = rig.run(&mut |guard, items| {
        let item = items
            .iter()
            .find(|i| i.kind == AttentionKind::Approval)
            .unwrap();
        asked.push((item.priority, item.detail.clone(), item.title.clone()));
        guard.approve(item.request.unwrap()).unwrap();
    });
    assert_eq!(out.len(), 3, "{out:?}");
    // Session start: the map is pushed.
    assert!(out[0].contains("SWARMUX KNOWLEDGE"), "{}", out[0]);
    assert!(out[0].contains("Product is a marketplace"), "{}", out[0]);
    // The read of a migration pulls the rules about migrations.
    let read: serde_json::Value = serde_json::from_str(&out[1]).unwrap();
    let ctx = read["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(ctx.contains("Migrations are append-only."), "{ctx}");
    assert!(ctx.contains("Anonymous users are full users."), "{ctx}");
    // The write that conditions on anonymous users waited for the owner.
    assert_eq!(asked.len(), 1);
    let (priority, detail, title) = &asked[0];
    assert_eq!(*priority, Priority::P1);
    assert!(
        detail.starts_with("knowledge rule R2: Anonymous users are full users."),
        "{detail}"
    );
    assert!(title.contains("002_guests.sql"), "{title}");
    let write: serde_json::Value = serde_json::from_str(&out[2]).unwrap();
    assert_eq!(
        write["hookSpecificOutput"]["permissionDecision"], "allow",
        "approved"
    );

    let entries = rig.knowledge_entries();
    assert!(
        entries
            .iter()
            .any(|(_, p)| p.contains("\"trigger\":\"startup\""))
    );
    assert!(
        entries
            .iter()
            .any(|(job, p)| job.is_some() && p.contains("\"trigger\":\"pull\""))
    );
    assert!(
        entries
            .iter()
            .any(|(_, p)| p.contains("\"event\":\"rule\"") && p.contains("\"action\":\"ask\""))
    );

    // The same rule in shadow: nothing is held, the would-be ask is logged.
    let r2 = ItemId::parse("R2").unwrap();
    rig.knowledge
        .set_status(&r2, Status::Shadow, "trial")
        .unwrap();
    let mut asked_again = 0;
    let out = rig.run(&mut |guard, items| {
        asked_again += 1;
        let item = &items[0];
        guard.deny(item.request.unwrap()).unwrap();
    });
    assert_eq!(asked_again, 0, "shadow rules don't hold calls");
    let write: serde_json::Value = serde_json::from_str(&out[2]).unwrap();
    assert_eq!(write["hookSpecificOutput"]["permissionDecision"], "allow");
    assert!(
        rig.knowledge_entries()
            .iter()
            .any(|(_, p)| p.contains("\"action\":\"would_ask\"") && p.contains("R2"))
    );
    assert_eq!(rig.knowledge.get(&r2).unwrap().counters.shadow_hits, 1);
}
