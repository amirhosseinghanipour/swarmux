//! Workspace missions: one job across the repositories a mission names,
//! checks run per repository, and a landing that is whole or nothing.

use std::path::{Path, PathBuf};

use super::*;
use crate::gate::GateState;
use crate::ports::RunnerMsg;
use crate::testing::{
    FakeJobGates, FakeMergeQueue, FakeRunner, InMemoryMissions, InMemoryWorkspaces, WorktreeRig,
};
use crate::workspace::WorkspaceStore as _;
use swarmux_domain::mission::{Acceptance, Budget, Constraints, LandState};
use swarmux_domain::{AgentEvent, AttentionKind, Limits, Repo, Session, Workspace, WorkspaceId};

struct W {
    rig: WorktreeRig,
    svc: MissionService,
    jobs: JobService,
    runner: Rc<FakeRunner>,
    merge: Rc<FakeMergeQueue>,
    gates: Rc<FakeJobGates>,
    session: Session,
}

fn repo(name: &str) -> Repo {
    Repo {
        name: name.into(),
        path: name.into(),
        git_root: Path::new("/p").join(name),
        default_branch: Some("main".into()),
        stacks: Vec::new(),
        commands: Vec::new(),
    }
}

fn rig() -> W {
    let rig = WorktreeRig::new();
    let runner = Rc::new(FakeRunner::default());
    let mut workspace = Workspace::new(WorkspaceId::random(), "Product", "/p".into(), 0).unwrap();
    workspace.set_repos(vec![repo("app"), repo("backend"), repo("admin")], 0);
    let workspaces = Rc::new(InMemoryWorkspaces::default());
    workspaces.save(&workspace).unwrap();
    let mut session =
        Session::new(SessionId::random(), "Product", "/p".into(), "sonnet", 0).unwrap();
    session.workspace = Some(workspace.id);
    crate::ports::SessionRepository::insert(&*rig.store, &session).unwrap();
    let jobs = JobService::new(
        rig.store.clone(),
        rig.store.clone(),
        runner.clone(),
        rig.clock.clone(),
    )
    .with_limits(Limits {
        max_concurrent: 4,
        min_free_ram: 0,
    })
    .with_worktrees(rig.worktrees.clone())
    .with_workspaces(workspaces.clone());
    let merge = Rc::new(FakeMergeQueue::default());
    let gates = Rc::new(FakeJobGates::default());
    let svc = MissionService::new(Rc::new(InMemoryMissions::default()), rig.clock.clone())
        .unwrap()
        .with_gates(gates.clone())
        .with_ledger(rig.ledger_service.clone())
        .with_workspaces(workspaces)
        .with_graph(merge.clone(), rig.worktrees.clone(), ScopeClaims::new());
    W {
        rig,
        svc,
        jobs,
        runner,
        merge,
        gates,
        session,
    }
}

fn contract(repos: &[&str], checks: &[&str]) -> Contract {
    Contract {
        title: "Guest checkout".into(),
        goal: "let guests check out with the new order endpoint".into(),
        repos: repos.iter().map(|r| r.to_string()).collect(),
        acceptance: Acceptance {
            checks: checks.iter().map(|c| c.to_string()).collect(),
            human_signoff: false,
        },
        constraints: Constraints {
            paths_rw: vec!["backend/**".into(), "app/**".into()],
            protected: Vec::new(),
        },
        budget: Budget {
            usd: Some(3.0),
            ..Budget::default()
        },
        clarifications: Vec::new(),
    }
}

fn pass(at: Millis) -> GateState {
    GateState {
        outcome: GateOutcome::Pass,
        blocks: 0,
        max_blocks: 3,
        checks: Vec::new(),
        protected_changed: 0,
        evidence_gaps: 0,
        hidden: None,
        tampering: 0,
        digest: String::new(),
        problem: None,
        at,
        checks_weakened: false,
        base_suite: None,
    }
}

impl W {
    fn create(&mut self, c: Contract) -> String {
        let ws = self.session.workspace;
        self.svc
            .create_in(c, "/p", Some(self.session.id), ws)
            .unwrap()
    }

    /// Runs the mission's job to a passing gate.
    fn deliver(&mut self, slug: &str) -> JobId {
        let job = self.svc.launch(slug, &mut self.jobs).unwrap();
        self.runner.emit(
            0,
            RunnerMsg::Event(AgentEvent::Result {
                is_error: false,
                subtype: "success".into(),
                cost_usd: 0.2,
                duration_ms: 900,
                num_turns: 1,
                tokens_in: 1,
                tokens_out: 1,
                cache_read: 0,
                context_window: None,
                subagents_spawned: 0,
                text: "done".into(),
                cache_creation: None,
            }),
        );
        self.runner.emit(
            0,
            RunnerMsg::Exited {
                code: Some(0),
                stderr_tail: String::new(),
            },
        );
        self.jobs.pump();
        let facts = self.jobs.facts();
        let at = self.rig.clock.now();
        let gates = HashMap::from([(job, pass(at))]);
        self.svc.pump(&facts, &gates);
        job
    }

    fn workdir(&self, repo: &str) -> PathBuf {
        PathBuf::from("/wt/integration").join(repo)
    }
}

#[test]
fn the_job_touches_only_the_named_repos_and_checks_run_in_them() {
    let mut w = rig();
    let slug = w.create(contract(
        &["backend", "app"],
        &["backend: cargo test", "app: flutter test"],
    ));
    assert_eq!(w.svc.get(&slug).unwrap().state, MissionState::Linted);
    let job = w.svc.launch(&slug, &mut w.jobs).unwrap();
    let set = w.rig.worktrees.set_for(job).unwrap();
    let names: Vec<_> = set.iter().filter_map(|r| r.repo_name()).collect();
    assert_eq!(names, ["backend", "app"], "admin gets no worktree");
    let req = w.runner.request(0);
    assert!(
        req.prompt.contains("- `cargo test` in `backend/`"),
        "{}",
        req.prompt
    );
    assert!(req.prompt.contains("producers first: backend, app"));
    let (assigned, gate) = w.gates.assigned.borrow()[0].clone();
    assert_eq!(assigned, job);
    let dirs: Vec<_> = gate.checks.iter().map(|c| c.dir.clone()).collect();
    assert_eq!(dirs, [Some("backend".to_string()), Some("app".to_string())]);
}

#[test]
fn a_mission_naming_a_repo_the_workspace_lacks_is_refused() {
    let mut w = rig();
    let slug = w.create(contract(&["web"], &["web: npm test"]));
    let lint = w.svc.get(&slug).unwrap().lint.clone().unwrap();
    assert!(lint.blocking());
    assert!(
        lint.findings
            .iter()
            .any(|f| f.kind == swarmux_domain::mission::FindingKind::UnknownRepo)
    );
    let e = w.svc.launch(&slug, &mut w.jobs).unwrap_err();
    assert!(e.to_string().contains("refused"), "{e}");
    assert_eq!(w.runner.started(), 0);
}

#[test]
fn a_passing_gate_lands_every_repo_in_order() {
    let mut w = rig();
    let slug = w.create(contract(&["backend", "app"], &["backend: cargo test"]));
    let job = w.deliver(&slug);
    let m = w.svc.get(&slug).unwrap();
    assert_eq!(m.state, MissionState::Delivered, "{:?}", m.note);
    let landing = m.landing.clone().unwrap();
    assert!(landing.complete());
    assert_eq!(landing.into, "swarmux/guest-checkout/integration");
    let order: Vec<_> = w
        .merge
        .merged_in
        .borrow()
        .iter()
        .map(|(dir, _)| dir.clone())
        .collect();
    assert_eq!(
        order,
        [w.workdir("backend"), w.workdir("app")],
        "producer first"
    );
    // Each repository's work was committed before it landed.
    assert!(w.rig.git.snapshots.get() >= 2);
    assert_eq!(
        w.gates.verified.borrow().len(),
        0,
        "the gate's own verdict was enough"
    );
    let _ = job;
}

#[test]
fn a_repo_that_cannot_land_puts_back_the_ones_before_it() {
    let mut w = rig();
    let slug = w.create(contract(&["backend", "app"], &["backend: cargo test"]));
    w.merge.conflict_in.borrow_mut().push(w.workdir("app"));
    w.deliver(&slug);
    let m = w.svc.get(&slug).unwrap().clone();
    assert_eq!(m.state, MissionState::Escalated);
    let landing = m.landing.clone().unwrap();
    let states: Vec<_> = landing.repos.iter().map(|r| r.state).collect();
    assert_eq!(states, [LandState::Undone, LandState::Failed]);
    assert!(
        w.merge.merged_in.borrow().is_empty(),
        "nothing stays landed: {:?}",
        w.merge.merged_in.borrow()
    );
    assert_eq!(w.merge.resets.borrow()[0].0, w.workdir("backend"));
    assert!(m.note.unwrap().contains("app could not land"));
    let items = w.svc.plan_items(w.rig.clock.now());
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].kind, AttentionKind::Merge);
    assert!(
        items[0].detail.contains("nothing landed"),
        "{}",
        items[0].detail
    );
}

#[test]
fn workspace_missions_build_in_one_set_not_a_graph() {
    let mut w = rig();
    let slug = w.create(contract(&[], &["cargo test"]));
    assert_eq!(
        w.svc.topology(&slug, &w.jobs),
        swarmux_domain::taskgraph::Topology::Single
    );
    let job = w.svc.launch(&slug, &mut w.jobs).unwrap();
    assert_eq!(
        w.rig.worktrees.set_for(job).unwrap().len(),
        3,
        "no repos named: all"
    );
}

// ---------------------------------------------------------------------
// landing = "manual": the gate passing does not land; the user does
// ---------------------------------------------------------------------

impl W {
    /// The same rig, landing by hand.
    fn manual(self) -> W {
        W {
            svc: self.svc.with_plan_settings(PlanSettings {
                landing: LandingPolicy::Manual,
                ..PlanSettings::default()
            }),
            ..self
        }
    }

    /// The user lands `slug`, as the gate stands for `job`.
    fn land(&mut self, slug: &str, job: JobId) -> AppResult<LandReport> {
        let facts = self.jobs.facts();
        let gates = HashMap::from([(job, pass(self.rig.clock.now()))]);
        self.svc.land_on_request(slug, &facts, &gates)
    }

    fn merged(&self) -> Vec<PathBuf> {
        self.merge
            .merged_in
            .borrow()
            .iter()
            .map(|(dir, _)| dir.clone())
            .collect()
    }
}

#[test]
fn manual_landing_waits_with_the_work_verified_until_the_user_lands_it() {
    let mut w = rig().manual();
    let slug = w.create(contract(&["backend", "app"], &["backend: cargo test"]));
    let job = w.deliver(&slug);
    // The gate passed, and nothing landed: the mission waits.
    let m = w.svc.get(&slug).unwrap();
    assert_eq!(m.state, MissionState::Verifying, "{:?}", m.note);
    assert!(m.landing.is_none());
    assert!(w.merged().is_empty());
    assert_eq!(w.rig.git.snapshots.get(), 0, "no repository was touched");
    // Pumping again, however often, does not land it either.
    let facts = w.jobs.facts();
    let gates = HashMap::from([(job, pass(w.rig.clock.now()))]);
    for _ in 0..3 {
        let report = w.svc.pump(&facts, &gates);
        assert!(!report.changed);
    }
    assert_eq!(w.svc.get(&slug).unwrap().state, MissionState::Verifying);
    assert!(w.merged().is_empty());

    // The user lands it: every repository, producers first, and the
    // mission is delivered, with the same ledger trail as automatic.
    assert_eq!(w.land(&slug, job).unwrap(), LandReport::Delivered);
    let m = w.svc.get(&slug).unwrap();
    assert_eq!(m.state, MissionState::Delivered, "{:?}", m.note);
    let landing = m.landing.clone().unwrap();
    assert!(landing.complete());
    assert_eq!(landing.into, "swarmux/guest-checkout/integration");
    assert_eq!(w.merged(), [w.workdir("backend"), w.workdir("app")]);
    assert!(w.rig.git.snapshots.get() >= 2);
    // Delivered work cannot be landed twice.
    let again = w.land(&slug, job).unwrap_err().to_string();
    assert!(again.contains("only work the gate passed"), "{again}");
    assert_eq!(w.merged().len(), 2);
}

#[test]
fn manual_landing_that_cannot_land_puts_back_what_it_landed_and_escalates() {
    let mut w = rig().manual();
    let slug = w.create(contract(&["backend", "app"], &["backend: cargo test"]));
    w.merge.conflict_in.borrow_mut().push(w.workdir("app"));
    let job = w.deliver(&slug);
    assert_eq!(w.svc.get(&slug).unwrap().state, MissionState::Verifying);

    let LandReport::Undone(why) = w.land(&slug, job).unwrap() else {
        panic!("the app repository conflicts");
    };
    assert!(why.contains("app could not land"), "{why}");
    let m = w.svc.get(&slug).unwrap().clone();
    assert_eq!(m.state, MissionState::Escalated);
    let states: Vec<_> = m.landing.unwrap().repos.iter().map(|r| r.state).collect();
    assert_eq!(states, [LandState::Undone, LandState::Failed]);
    // Undo: backend was merged first and is back where it was.
    assert!(w.merged().is_empty(), "{:?}", w.merged());
    assert_eq!(w.merge.resets.borrow().len(), 1);
    assert_eq!(w.merge.resets.borrow()[0].0, w.workdir("backend"));
    // The user is told, as for an automatic landing.
    let items = w.svc.plan_items(w.rig.clock.now());
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].kind, AttentionKind::Merge);
}

#[test]
fn landing_on_request_is_refused_where_there_is_nothing_to_land() {
    let mut w = rig().manual();
    // The gate has not passed: the job has not even run.
    let slug = w.create(contract(&["backend"], &["backend: cargo test"]));
    let job = w.svc.launch(&slug, &mut w.jobs).unwrap();
    let e = w.land(&slug, job).unwrap_err().to_string();
    assert!(e.contains("only work the gate passed"), "{e}");
    assert!(w.merged().is_empty());

    // A mission worked in one folder has no integration branch.
    let session = w.session.id;
    let one = w
        .svc
        .create(contract(&[], &["cargo test"]), "/p", Some(session))
        .unwrap();
    let e = w.land(&one, job).unwrap_err().to_string();
    assert!(e.contains("one folder"), "{e}");
    let e = w.land("no-such-mission", job).unwrap_err().to_string();
    assert!(e.contains("no-such-mission"), "{e}");
}

#[test]
fn a_failing_gate_is_not_landed_even_on_request() {
    let mut w = rig().manual();
    let slug = w.create(contract(&["backend"], &["backend: cargo test"]));
    let job = w.svc.launch(&slug, &mut w.jobs).unwrap();
    w.runner.emit(
        0,
        RunnerMsg::Event(AgentEvent::Result {
            is_error: false,
            subtype: "success".into(),
            cost_usd: 0.2,
            duration_ms: 900,
            num_turns: 1,
            tokens_in: 1,
            tokens_out: 1,
            cache_read: 0,
            context_window: None,
            subagents_spawned: 0,
            text: "done".into(),
            cache_creation: None,
        }),
    );
    w.runner.emit(
        0,
        RunnerMsg::Exited {
            code: Some(0),
            stderr_tail: String::new(),
        },
    );
    w.jobs.pump();
    let facts = w.jobs.facts();
    let mut blocked = pass(w.rig.clock.now());
    blocked.outcome = GateOutcome::Block;
    let gates = HashMap::from([(job, blocked)]);
    w.svc.pump(&facts, &gates);
    assert_eq!(w.svc.get(&slug).unwrap().state, MissionState::Verifying);
    let e = w
        .svc
        .land_on_request(&slug, &facts, &gates)
        .unwrap_err()
        .to_string();
    assert!(e.contains("gate has not passed"), "{e}");
    assert!(w.merged().is_empty());
}

#[test]
fn auto_is_the_default_and_lands_at_the_gate_as_before() {
    assert_eq!(PlanSettings::default().landing, LandingPolicy::Auto);
    let mut w = rig();
    let slug = w.create(contract(&["backend", "app"], &["backend: cargo test"]));
    let job = w.deliver(&slug);
    assert_eq!(
        w.svc.get(&slug).unwrap().state,
        MissionState::Delivered,
        "no user step in auto mode"
    );
    assert_eq!(w.merged(), [w.workdir("backend"), w.workdir("app")]);
    // Asking to land what is already delivered is refused, not repeated.
    assert!(w.land(&slug, job).is_err());
    assert_eq!(w.merged().len(), 2);
}
