//! Task graphs: an approved plan built as parallel jobs, inside their
//! claims, merged in order. The acceptance suites live here: planted
//! out-of-scope edits, the speedup on a decomposable plan, and a conflict.

use std::sync::Arc;

use super::*;
use crate::gate::GateState;
use crate::guard::SharedPolicy;
use crate::ports::{HookRequest, RunnerMsg};
use crate::testing::{
    FakeJobGates, FakeMergeQueue, FakePlanner, FakeRunner, InMemoryMissions, InMemoryPlans,
    WorktreeRig,
};
use swarmux_domain::mission::{Acceptance, Budget, Constraints};
use swarmux_domain::plan::{Plan, PlanTask};
use swarmux_domain::taskgraph::{TaskState, Topology};
use swarmux_domain::{AgentEvent, AttentionKind, GuardPolicy, Limits, Priority, Verdict};

mod restart;

const MIN: Millis = 60_000;

struct G {
    rig: WorktreeRig,
    svc: MissionService,
    jobs: JobService,
    runner: Rc<FakeRunner>,
    planner: Rc<FakePlanner>,
    merge: Rc<FakeMergeQueue>,
    claims: Arc<ScopeClaims>,
    gates: Rc<FakeJobGates>,
    policy: SharedPolicy,
    /// Gate verdicts the test hands the pump.
    verdicts: HashMap<JobId, GateState>,
    /// Runner requests already finished.
    finished: Vec<usize>,
    /// Where missions and plans outlive a restart.
    missions_store: Rc<InMemoryMissions>,
    plans_store: Rc<InMemoryPlans>,
}

fn task(id: &str, files: &[&str], deps: &[&str]) -> PlanTask {
    PlanTask {
        id: id.into(),
        title: format!("task {id}"),
        files: files.iter().map(|s| s.to_string()).collect(),
        depends_on: deps.iter().map(|s| s.to_string()).collect(),
        acceptance: "cargo test".into(),
    }
}

/// Four independent tasks and one that needs them all.
fn decomposable() -> Plan {
    Plan {
        summary: "split the app into modules, then wire them".into(),
        tasks: vec![
            task("t1", &["src/api/**"], &[]),
            task("t2", &["src/db/**"], &[]),
            task("t3", &["src/ui/**", "tests/ui.rs"], &[]),
            task("t4", &["docs/guide.md"], &[]),
            task("t5", &["src/main.rs"], &["t1", "t2", "t3", "t4"]),
        ],
    }
}

fn rig(max_concurrent: usize) -> G {
    let rig = WorktreeRig::new();
    let runner = Rc::new(FakeRunner::default());
    let jobs = JobService::new(
        rig.store.clone(),
        rig.store.clone(),
        runner.clone(),
        rig.clock.clone(),
    )
    .with_limits(Limits {
        max_concurrent,
        min_free_ram: 0,
    })
    .with_worktrees(rig.worktrees.clone());
    let planner = Rc::new(FakePlanner::default());
    let merge = Rc::new(FakeMergeQueue::default());
    let claims = ScopeClaims::new();
    let gates = Rc::new(FakeJobGates::default());
    let missions_store = Rc::new(InMemoryMissions::default());
    let plans_store = Rc::new(InMemoryPlans::default());
    let svc = MissionService::new(missions_store.clone(), rig.clock.clone())
        .unwrap()
        .with_gates(gates.clone())
        .with_ledger(rig.ledger_service.clone())
        .with_planner(planner.clone(), plans_store.clone())
        .unwrap()
        .with_plan_settings(PlanSettings {
            plan_first: true,
            ..PlanSettings::default()
        })
        .with_graph(merge.clone(), rig.worktrees.clone(), claims.clone());
    let policy = SharedPolicy::new(GuardPolicy::default()).with_fence(claims.clone());
    G {
        rig,
        svc,
        jobs,
        runner,
        planner,
        merge,
        claims,
        gates,
        policy,
        verdicts: HashMap::new(),
        finished: Vec::new(),
        missions_store,
        plans_store,
    }
}

fn contract() -> Contract {
    Contract {
        title: "Split the app".into(),
        goal: "split the app into api, db and ui modules and wire them in main".into(),
        acceptance: Acceptance {
            checks: vec!["cargo test".into()],
            human_signoff: false,
        },
        constraints: Constraints {
            paths_rw: vec!["src/**".into(), "docs/**".into()],
            protected: vec!["tests/**".into()],
        },
        budget: Budget {
            usd: Some(20.0),
            ..Budget::default()
        },
        clarifications: Vec::new(),
    }
}

impl G {
    /// A mission with `plan` approved and its graph started.
    fn approved(&mut self, plan: Plan) -> String {
        let session = Some(self.rig.session.id);
        let slug = self.svc.create(contract(), "/repo", session).unwrap();
        assert_eq!(
            self.svc.begin(&slug, &mut self.jobs).unwrap(),
            Begun::Planning
        );
        self.planner.reply(
            0,
            PlanReply {
                result: Ok(plan),
                cost_usd: 0.1,
            },
        );
        self.tick();
        self.svc.approve_plan(&slug, &mut self.jobs).unwrap();
        slug
    }

    fn tick(&mut self) -> MissionReport {
        self.jobs.pump();
        let facts = self.jobs.facts();
        let mut report = self.svc.pump(&facts, &self.verdicts);
        let g = self.svc.pump_graphs(&mut self.jobs, &facts, &self.verdicts);
        for job in &g.cancel {
            let _ = self.jobs.cancel(*job);
        }
        report.errors.extend(g.errors);
        report.changed |= g.changed;
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        report
    }

    fn run(&self, slug: &str) -> swarmux_domain::taskgraph::GraphRun {
        self.svc.graph(slug).cloned().unwrap()
    }

    fn state(&self, slug: &str, id: &str) -> TaskState {
        self.run(slug).task(id).unwrap().state
    }

    fn job_of(&self, slug: &str, id: &str) -> JobId {
        self.run(slug).task(id).unwrap().job.unwrap()
    }

    fn branch_of(&self, job: JobId) -> String {
        self.rig.worktrees.record_for(job).unwrap().unwrap().branch
    }

    fn root_of(&self, job: JobId) -> String {
        self.rig
            .worktrees
            .record_for(job)
            .unwrap()
            .unwrap()
            .path
            .display()
            .to_string()
            .replace('\\', "/")
    }

    /// The runner request of `job`.
    fn index_of(&self, job: JobId) -> usize {
        (0..self.runner.started())
            .rev()
            .find(|&i| self.runner.request(i).job_id == job)
            .unwrap()
    }

    fn finish(&mut self, job: JobId, cost: f64) {
        let i = self.index_of(job);
        self.finished.push(i);
        self.runner.emit(
            i,
            RunnerMsg::Event(AgentEvent::Result {
                is_error: false,
                subtype: "success".into(),
                cost_usd: cost,
                duration_ms: 1,
                num_turns: 1,
                tokens_in: 1,
                tokens_out: 1,
                cache_read: 0,
                context_window: None,
                subagents_spawned: 0,
                text: "done".into(),
            }),
        );
        self.runner.emit(
            i,
            RunnerMsg::Exited {
                code: Some(0),
                stderr_tail: String::new(),
            },
        );
    }

    /// A fake agent's tool call through the Guard, as the hook sends it.
    /// What the Guard lets through is written to the task's branch.
    fn agent_writes(&self, job: JobId, tool: &str, subject: &str, file: &str) -> bool {
        let cwd = self.root_of(job);
        let (d, _) = self.policy.decide(&HookRequest {
            request_id: 1,
            job_id: Some(job),
            tool_name: tool.into(),
            subject: subject.into(),
            input: String::new(),
            cwd,
        });
        let refused = d.verdict == Verdict::Deny;
        if refused {
            assert!(
                d.rule.as_deref().is_some_and(|r| r.contains("scope claim")),
                "{d:?}"
            );
        } else {
            self.merge.touch(&self.branch_of(job), file);
        }
        !refused
    }

    /// swarmux stops and starts again: processes, the runner and every
    /// service's memory are gone; the store, the worktrees, the plans and
    /// git stay. Returns what the recovery reported.
    fn restart(&mut self) -> MissionReport {
        let runner = Rc::new(FakeRunner::default());
        let mut jobs = JobService::new(
            self.rig.store.clone(),
            self.rig.store.clone(),
            runner.clone(),
            self.rig.clock.clone(),
        )
        .with_limits(Limits {
            max_concurrent: 4,
            min_free_ram: 0,
        })
        .with_worktrees(self.rig.worktrees.clone());
        jobs.recover().unwrap();
        let claims = ScopeClaims::new();
        let gates = Rc::new(FakeJobGates::default());
        let planner = Rc::new(FakePlanner::default());
        let mut svc = MissionService::new(self.missions_store.clone(), self.rig.clock.clone())
            .unwrap()
            .with_gates(gates.clone())
            .with_ledger(self.rig.ledger_service.clone())
            .with_planner(planner.clone(), self.plans_store.clone())
            .unwrap()
            .with_plan_settings(PlanSettings {
                plan_first: true,
                ..PlanSettings::default()
            })
            .with_graph(
                self.merge.clone(),
                self.rig.worktrees.clone(),
                claims.clone(),
            );
        let report = svc.recover(&mut jobs);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        self.policy = SharedPolicy::new(GuardPolicy::default()).with_fence(claims.clone());
        self.svc = svc;
        self.jobs = jobs;
        self.runner = runner;
        self.claims = claims;
        self.gates = gates;
        self.planner = planner;
        self.verdicts.clear();
        self.finished.clear();
        report
    }

    /// The job's agent reports its session: it has begun and can be resumed.
    fn begins(&mut self, job: JobId) {
        let i = self.index_of(job);
        self.runner.emit(
            i,
            RunnerMsg::Event(AgentEvent::Init {
                session_id: "claude-session".into(),
                model: "sonnet".into(),
                cwd: String::new(),
                permission_mode: "default".into(),
            }),
        );
        self.jobs.pump();
    }

    /// Hands the final job a passing verdict on the integrated result.
    fn pass_final(&mut self, slug: &str) {
        let job = self.run(slug).final_job.unwrap();
        self.rig.clock.advance(1);
        self.verdicts.insert(job, pass(self.rig.clock.now()));
    }

    fn ledger(&self, event: &str) -> Vec<String> {
        let key = format!(r#""plan":"{event}""#);
        self.rig
            .ledger
            .entries()
            .into_iter()
            .map(|e| e.draft.payload_json)
            .filter(|p| p.contains(&key))
            .collect()
    }
}

fn pass(at: Millis) -> GateState {
    GateState {
        outcome: GateOutcome::Pass,
        blocks: 0,
        max_blocks: 3,
        checks: vec![("cargo test".into(), true, false)],
        protected_changed: 0,
        evidence_gaps: 0,
        hidden: None,
        tampering: 0,
        digest: String::new(),
        problem: None,
        at,
    }
}

#[test]
fn a_decomposable_plan_fans_out_and_merges_in_order() {
    let mut g = rig(4);
    let slug = g.approved(decomposable());
    assert_eq!(g.svc.topology(&slug, &g.jobs), Topology::Fanout);
    let run = g.run(&slug);
    assert_eq!(run.integration, "swarmux/split-the-app/integration");
    assert_eq!(g.merge.opened.borrow()[0].1, run.integration);
    // Four tasks out at once, each in its own session and worktree,
    // branched from the integration branch, each claiming its files.
    assert_eq!(g.runner.started(), 4);
    for id in ["t1", "t2", "t3", "t4"] {
        assert_eq!(g.state(&slug, id), TaskState::Running, "{id}");
    }
    assert_eq!(g.state(&slug, "t5"), TaskState::Waiting);
    let sessions: std::collections::HashSet<_> =
        (0..4).map(|i| g.runner.request(i).session_id).collect();
    assert_eq!(sessions.len(), 4);
    assert!(
        g.rig
            .git
            .bases
            .borrow()
            .iter()
            .all(|(_, b)| b.as_deref() == Some(run.integration.as_str()))
    );
    let prompt = g.runner.request(0).prompt;
    assert!(prompt.contains("Your task: t1"), "{prompt}");
    assert!(
        prompt.contains("you may change only src/api/**"),
        "{prompt}"
    );
    assert_eq!(g.claims.held().len(), 4);
    assert_eq!(g.ledger("claim").len(), 4);
    // Task gates protect the oracles but leave the mission's checks for
    // the integrated result.
    let assigned = g.gates.assigned.borrow().clone();
    assert!(assigned.iter().all(|(_, gate)| gate.checks.is_empty()));
    g.tick();
    assert_eq!(g.svc.get(&slug).unwrap().state, MissionState::Running);

    for id in ["t3", "t1", "t4", "t2"] {
        let job = g.job_of(&slug, id);
        g.finish(job, 0.5);
        g.tick();
    }
    // Merged as each finished; t5 starts on top of all four.
    assert_eq!(g.run(&slug).merged, vec!["t3", "t1", "t4", "t2"]);
    assert_eq!(g.state(&slug, "t5"), TaskState::Running);
    let t5 = g.job_of(&slug, "t5");
    g.finish(t5, 0.5);
    g.tick();
    let run = g.run(&slug);
    assert!(run.all_merged());
    assert_eq!(run.final_job, Some(t5));
    // The last branch is brought up to the integration branch and checked.
    assert_eq!(g.merge.forwarded.borrow()[0].1, run.integration);
    assert_eq!(g.gates.verified.borrow().last().unwrap().0, t5);
    let m = g.svc.get(&slug).unwrap();
    assert_eq!(m.state, MissionState::Verifying);
    assert_eq!(m.job, Some(t5));
    assert!(
        (m.spend.implement - 2.5).abs() < 1e-9,
        "{}",
        m.spend.implement
    );
    g.pass_final(&slug);
    g.tick();
    assert_eq!(g.svc.get(&slug).unwrap().state, MissionState::Delivered);
    assert!(g.claims.held().is_empty());
    assert_eq!(g.ledger("merged").len(), 5);
    assert_eq!(g.ledger("integrated").len(), 1);
}

#[test]
fn a_chain_builds_as_one_job() {
    let mut g = rig(4);
    let chain = Plan {
        summary: "s".into(),
        tasks: vec![
            task("t1", &["src/a.rs"], &[]),
            task("t2", &["src/b.rs"], &["t1"]),
        ],
    };
    let slug = g.approved(chain);
    assert!(g.svc.graph(&slug).is_none());
    assert_eq!(g.runner.started(), 1);
    assert!(g.runner.request(0).prompt.contains("Approved plan:"));
}

/// Acceptance (a): fake agents try to write outside their claims; none of
/// it lands in the integration branch.
#[test]
fn planted_out_of_scope_edits_never_land() {
    let mut g = rig(4);
    let slug = g.approved(decomposable());
    g.tick();
    let t = |g: &G, id: &str| g.job_of(&slug, id);
    let (j1, j2, j3, j4) = (t(&g, "t1"), t(&g, "t2"), t(&g, "t3"), t(&g, "t4"));
    let user = "/repo/src/api/handlers.rs";
    // (job, tool, subject, file it would change, in scope)
    let wt1 = g.root_of(j1);
    let planted: Vec<(JobId, &str, String, &str, bool)> = vec![
        (
            j1,
            "Edit",
            "src/api/handlers.rs".into(),
            "src/api/handlers.rs",
            true,
        ),
        (
            j1,
            "Write",
            format!("{wt1}/src/api/mod.rs"),
            "src/api/mod.rs",
            true,
        ),
        (j1, "Edit", "src/db/pool.rs".into(), "src/db/pool.rs", false),
        (j1, "Write", user.into(), "src/api/handlers.rs", false),
        (
            j1,
            "Bash",
            "echo x > src/main.rs".into(),
            "src/main.rs",
            false,
        ),
        (
            j2,
            "MultiEdit",
            "src/db/pool.rs".into(),
            "src/db/pool.rs",
            true,
        ),
        (
            j2,
            "Edit",
            "src/api/../ui/view.rs".into(),
            "src/ui/view.rs",
            false,
        ),
        (
            j2,
            "Bash",
            "rm -f docs/guide.md".into(),
            "docs/guide.md",
            false,
        ),
        (j2, "Write", "Cargo.toml".into(), "Cargo.toml", false),
        (j3, "Edit", "src/ui/view.rs".into(), "src/ui/view.rs", true),
        (
            j3,
            "Bash",
            "cp src/ui/view.rs src/db/view.rs".into(),
            "src/db/view.rs",
            false,
        ),
        (
            j3,
            "NotebookEdit",
            "notes/x.ipynb".into(),
            "notes/x.ipynb",
            false,
        ),
        (j4, "Write", "docs/guide.md".into(), "docs/guide.md", true),
        (j4, "Edit", "README.md".into(), "README.md", false),
        (
            j4,
            "Bash",
            "tee src/api/x.rs < /dev/null".into(),
            "src/api/x.rs",
            false,
        ),
    ];
    let mut tried_out = 0;
    let mut denied = 0;
    for (job, tool, subject, file, in_scope) in &planted {
        let through = g.agent_writes(*job, tool, subject, file);
        assert_eq!(through, *in_scope, "{tool} {subject}");
        if !in_scope {
            tried_out += 1;
            denied += u32::from(!through);
        }
    }
    // One write the Guard can't see (a script writing a file) gets into
    // t3's branch anyway: the merge refuses the branch.
    g.merge.touch(&g.branch_of(j3), "src/db/schema.sql");
    tried_out += 1;
    for job in [j1, j2, j3, j4] {
        g.finish(job, 0.1);
        g.tick();
    }
    let run = g.run(&slug);
    assert_eq!(run.denials, denied);
    assert_eq!(g.ledger("scope_denied").len() as u32, denied);
    assert_eq!(g.state(&slug, "t3"), TaskState::Failed);
    assert!(run.task("t3").unwrap().note.contains("src/db/schema.sql"));
    assert_eq!(g.ledger("scope_blocked").len(), 1);
    let landed = g.merge.landed.borrow().clone();
    let out_of_scope: Vec<&String> = landed
        .iter()
        .filter(|f| {
            !planted
                .iter()
                .any(|(_, _, _, file, ok)| *ok && file == &f.as_str())
        })
        .collect();
    println!(
        "scope suite: {tried_out} out-of-scope writes planted, {denied} refused in PreToolUse, \
         {} refused at merge, {} landed; {} in-scope files landed",
        tried_out - denied,
        out_of_scope.len(),
        landed.len()
    );
    assert!(out_of_scope.is_empty(), "{out_of_scope:?}");
    assert_eq!(tried_out, 11);
    assert_eq!(denied, 10);
    // The blocked task asks the user; t5 can't start, so the mission
    // escalates once nothing else moves.
    let items = g.svc.plan_items(g.rig.clock.now());
    let item = items
        .iter()
        .find(|i| i.kind == AttentionKind::Merge)
        .unwrap();
    assert_eq!(item.priority, Priority::P2);
    assert!(item.detail.contains("outside its scope"), "{}", item.detail);
    assert_eq!(g.svc.get(&slug).unwrap().state, MissionState::Escalated);
}

#[test]
fn a_task_follows_its_work_into_a_restarted_job() {
    let mut g = rig(4);
    let slug = g.approved(decomposable());
    g.tick();
    let old = g.job_of(&slug, "t2");
    let branch = g.branch_of(old);
    // The watchdog hands the work to a fresh session in the same worktree.
    let (new, _) = g
        .jobs
        .restart(
            old,
            crate::Restart {
                prompt: "carry on".into(),
                model: None,
            },
        )
        .unwrap();
    g.tick();
    assert_eq!(g.job_of(&slug, "t2"), new);
    assert_eq!(g.state(&slug, "t2"), TaskState::Running);
    assert_eq!(g.branch_of(new), branch);
    assert!(g.claims.get(old).is_none());
    assert_eq!(g.claims.get(new).unwrap().task, "t2");
    assert!(g.agent_writes(new, "Edit", "src/db/x.rs", "src/db/x.rs"));
    assert!(!g.agent_writes(new, "Edit", "src/api/x.rs", "src/api/x.rs"));
    assert!(g.gates.assigned.borrow().iter().any(|(j, _)| *j == new));
}

#[test]
fn writes_before_the_worktree_is_known_are_held() {
    let claims = ScopeClaims::new();
    claims.claim(
        JobId(7),
        Claim {
            mission: "m".into(),
            task: "t1".into(),
            areas: vec!["src/**".into()],
            root: None,
        },
    );
    use crate::gate::WriteFence;
    let why = claims
        .refuses(Some(JobId(7)), "Edit", "src/a.rs", "/wt/x")
        .unwrap();
    assert!(why.contains("setting up"), "{why}");
    assert_eq!(
        claims.refuses(Some(JobId(7)), "Read", "src/a.rs", "/wt/x"),
        None
    );
    assert_eq!(
        claims.refuses(Some(JobId(8)), "Edit", "x.rs", "/wt/x"),
        None
    );
    assert!(claims.set_root(JobId(7), "/wt/x"));
    assert_eq!(
        claims.refuses(Some(JobId(7)), "Edit", "src/a.rs", "/wt/x"),
        None
    );
    assert!(claims.take_denials().is_empty());
}

/// Runs `plan` to delivery with each task's job taking its scripted
/// minutes, and returns the makespan in minutes.
fn makespan(max_concurrent: usize, plan: Plan, minutes: &[(&str, i64)]) -> i64 {
    let mut g = rig(max_concurrent);
    let start = g.rig.clock.now();
    let slug = g.approved(plan);
    let mut started_at: HashMap<usize, Millis> = HashMap::new();
    for _ in 0..1_000 {
        g.tick();
        let now = g.rig.clock.now();
        let m = g.svc.get(&slug).unwrap();
        if m.state == MissionState::Delivered {
            return (now - start) / MIN;
        }
        assert!(!m.state.is_final(), "{:?}", m.note);
        if m.state == MissionState::Verifying && !g.verdicts.contains_key(&m.job.unwrap()) {
            g.pass_final(&slug);
            continue;
        }
        let run = g.run(&slug);
        for i in 0..g.runner.started() {
            let at = *started_at.entry(i).or_insert(now);
            if g.finished.contains(&i) {
                continue;
            }
            let job = g.runner.request(i).job_id;
            let Some(task) = run.task_of(job) else {
                continue;
            };
            let mins = minutes.iter().find(|(id, _)| *id == task.id).unwrap().1;
            if now - at >= mins * MIN {
                g.finish(job, 0.2);
            }
        }
        g.rig.clock.advance(MIN);
    }
    panic!("the mission never finished");
}

/// Acceptance (b): four independent tasks and one dependent, with scripted
/// durations, on a test clock: parallel against serial.
#[test]
fn a_decomposable_plan_builds_at_least_1_8_times_faster() {
    let minutes = [("t1", 12), ("t2", 10), ("t3", 8), ("t4", 10), ("t5", 6)];
    let serial = makespan(1, decomposable(), &minutes);
    let parallel = makespan(4, decomposable(), &minutes);
    let speedup = serial as f64 / parallel as f64;
    println!(
        "speedup suite: serial {serial} min, parallel {parallel} min, speedup {speedup:.2}x \
         (sum of task time {} min)",
        minutes.iter().map(|(_, m)| m).sum::<i64>()
    );
    assert!(serial >= 46, "{serial}");
    assert!(speedup >= 1.8, "{speedup:.2}");
}

/// Acceptance (c): a conflict sends the agent back once; a second one
/// goes to the user.
#[test]
fn a_conflict_reinvokes_once_then_asks_the_user() {
    let mut g = rig(4);
    let plan = Plan {
        summary: "two halves".into(),
        tasks: vec![
            task("t1", &["src/a/**"], &[]),
            task("t2", &["src/b/**"], &[]),
        ],
    };
    let slug = g.approved(plan.clone());
    let (j1, j2) = (g.job_of(&slug, "t1"), g.job_of(&slug, "t2"));
    let b2 = g.branch_of(j2);
    g.merge.touch(&g.branch_of(j1), "src/a/x.rs");
    g.merge.touch(&b2, "src/b/y.rs");
    g.merge
        .conflicts
        .borrow_mut()
        .insert(b2.clone(), (2, vec!["src/b/y.rs".into()]));
    g.finish(j1, 0.1);
    g.tick();
    g.finish(j2, 0.1);
    g.tick();
    // Sent back once, in the same worktree, holding the same claim.
    assert_eq!(g.state(&slug, "t2"), TaskState::Conflict);
    let again = g.job_of(&slug, "t2");
    assert_ne!(again, j2);
    assert_eq!(g.branch_of(again), b2);
    assert_eq!(g.claims.get(again).unwrap().task, "t2");
    assert!(g.claims.get(j2).is_none());
    let brief = g.runner.request(g.index_of(again)).prompt;
    assert!(
        brief.contains("git rebase swarmux/split-the-app/integration"),
        "{brief}"
    );
    assert!(g.svc.plan_items(g.rig.clock.now()).is_empty());
    g.finish(again, 0.1);
    g.tick();
    // Still conflicting: no second re-invoke, the user is asked.
    let reinvokes = (0..g.runner.started())
        .filter(|&i| g.runner.request(i).prompt.contains("could not merge"))
        .count();
    assert_eq!(reinvokes, 1);
    assert_eq!(g.state(&slug, "t2"), TaskState::Failed);
    let items = g.svc.plan_items(g.rig.clock.now());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, AttentionKind::Merge);
    assert!(
        items[0].detail.contains("still conflicts"),
        "{}",
        items[0].detail
    );
    assert_eq!(g.svc.get(&slug).unwrap().state, MissionState::Escalated);
    assert_eq!(g.merge.merged.borrow().len(), 1, "only t1 merged");
    assert_eq!(g.ledger("conflict").len(), 2);
    println!(
        "conflict suite: {} merge attempts for t2, {reinvokes} re-invoke, {} inbox item",
        g.merge.attempts.get() - 1,
        items.len()
    );

    // With one conflict only, the rebase fixes it and the mission delivers.
    let mut g = rig(4);
    let slug = g.approved(plan);
    let (j1, j2) = (g.job_of(&slug, "t1"), g.job_of(&slug, "t2"));
    let b2 = g.branch_of(j2);
    g.merge
        .conflicts
        .borrow_mut()
        .insert(b2, (1, vec!["src/b/y.rs".into()]));
    g.finish(j1, 0.1);
    g.finish(j2, 0.1);
    g.tick();
    let again = g.job_of(&slug, "t2");
    g.finish(again, 0.1);
    g.tick();
    assert_eq!(g.state(&slug, "t2"), TaskState::Merged);
    g.pass_final(&slug);
    g.tick();
    assert_eq!(g.svc.get(&slug).unwrap().state, MissionState::Delivered);
}
