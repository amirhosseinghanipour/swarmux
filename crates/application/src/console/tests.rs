//! The console read model over literals and the in-memory fakes: where
//! outcomes land, what a hold quotes, what the Deck relates, and the
//! shape of the JSON.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use swarmux_domain::board::{BoardEntry, BoardKind};
use swarmux_domain::contracts::{Fact, FileFacts, Index, Symbol};
use swarmux_domain::evidence::{
    Bundle, ChangeKind, CheckResult, DiffFile, DiffSummary, GateRef, LedgerRange, Signature,
};
use swarmux_domain::gate::GateOutcome;
use swarmux_domain::mission::{
    Acceptance, Budget, Constraints, Contract, LandState, Landing, Mission, MissionState, Spend,
};
use swarmux_domain::plan::{DriftReport, Plan, PlanRecord, PlanStatus, PlanTask};
use swarmux_domain::taskgraph::{GraphRun, TaskRun, TaskState, Topology};
use swarmux_domain::{
    AttentionItem, HeldCall, JobId, JobStatus, Millis, ReviewState, Session, SessionId, WorkspaceId,
};

use super::*;
use crate::gate::{GateDetail, GateRun, GateState};
use crate::jobs::{JobFact, QueueStats};
use crate::missions::LandingPolicy;
use crate::ports::{HookRequest, JobRepository, NewJob, SessionRepository};
use crate::testing::InMemoryStore;
use crate::{DecisionRecord, Outcome, PendingApproval, Source, Stored};

const NOW: Millis = 10_000_000;

fn sid(n: u8) -> SessionId {
    SessionId::parse(&format!("5eed0000-0000-4000-8000-{n:012}")).unwrap()
}

fn fact(id: i64, session: u8, name: &str, status: JobStatus) -> JobFact {
    JobFact {
        job_id: JobId(id),
        session_id: sid(session),
        session_name: name.into(),
        prompt: format!("job {id}: {name}"),
        model: "sonnet".into(),
        status,
        created_at: 1_000_000,
        started_at: Some(1_100_000),
        last_event_at: None,
        finished_at: None,
        error: None,
        result: None,
        review: None,
        review_of: None,
        cost_usd: 0.5,
        context: Some(0.25),
        cache_ratio: None,
        flow: Vec::new(),
        todos_done: 0,
        todos_total: 0,
        loop_stops: 0,
        agents_running: 0,
        session_state: None,
    }
}

fn contract(title: &str) -> Contract {
    Contract {
        title: title.into(),
        goal: format!("{title}, as the spec says"),
        repos: Vec::new(),
        acceptance: Acceptance {
            checks: vec!["cargo test".into()],
            human_signoff: false,
        },
        constraints: Constraints {
            paths_rw: vec!["src/**".into()],
            protected: vec!["tests/**".into()],
        },
        budget: Budget {
            usd: Some(5.0),
            wall_minutes: Some(90),
            ..Budget::default()
        },
        due: None,
        clarifications: Vec::new(),
    }
}

fn mission(slug: &str, state: MissionState, updated_at: Millis) -> Mission {
    let mut m = Mission::new(
        slug,
        "/work/repo",
        contract(&slug.replace('-', " ")),
        900_000,
    );
    m.state = state;
    m.updated_at = updated_at;
    m
}

fn run_of(name: &str, passed: bool, ms: u64, first_failure: Option<&str>) -> GateRun {
    GateRun {
        name: name.into(),
        command: name.into(),
        exit: Some(i32::from(!passed)),
        timed_out: false,
        spawn_error: None,
        duration_ms: ms,
        flaky: false,
        passed,
        first_failure: first_failure.map(str::to_string),
    }
}

fn gate_state(outcome: GateOutcome, checks: &[(&str, bool)], at: Millis) -> GateState {
    GateState {
        outcome,
        blocks: 1,
        max_blocks: 3,
        checks: checks
            .iter()
            .map(|(n, ok)| (n.to_string(), *ok, false))
            .collect(),
        protected_changed: 0,
        evidence_gaps: 0,
        hidden: None,
        tampering: 0,
        checks_weakened: false,
        base_suite: None,
        digest: String::new(),
        problem: None,
        at,
        notes: Default::default(),
    }
}

fn stored_bundle() -> Stored {
    let mut b = Bundle {
        schema: 1,
        job: 1,
        repo: "/work/api-gateway".into(),
        head_sha: "9f2c1d7".into(),
        sealed_at: 2_100_000,
        diff: DiffSummary {
            files: 3,
            insertions: 20,
            deletions: 4,
            hash: "d1ff".into(),
            changes: vec![
                DiffFile::text("src/etl.rs", 12, 2).with_kind(ChangeKind::Modified),
                DiffFile::text("src/etl/counts.rs", 5, 1).with_kind(ChangeKind::Modified),
                DiffFile::text("docs/etl.md", 3, 1).with_kind(ChangeKind::Modified),
            ],
        },
        checks: vec![CheckResult {
            name: "cargo test".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            timeout_secs: 600,
            exit: Some(0),
            passed: true,
            flaky: false,
            duration_ms: 4_200,
            first_failure: String::new(),
        }],
        gates: vec![
            GateRef {
                seq: 30,
                outcome: "block".into(),
                event: "Stop".into(),
                blocks: 0,
                digest: "aa".into(),
            },
            GateRef {
                seq: 38,
                outcome: "pass".into(),
                event: "Stop".into(),
                blocks: 1,
                digest: "bb".into(),
            },
        ],
        ledger: LedgerRange {
            from_seq: 10,
            to_seq: 42,
            head_hash: "ab12cd".into(),
        },
        ..Bundle::default()
    };
    let digest = b.digest_hex();
    b.signature = Some(Signature {
        alg: "sha256".into(),
        value: digest.clone(),
    });
    Stored {
        bundle: b,
        digest,
        seal_seq: Some(43),
        labels: Vec::new(),
    }
}

/// Everything a snapshot reads, as literals.
struct Rig {
    now: Millis,
    store: InMemoryStore,
    facts: Vec<JobFact>,
    attention: Vec<AttentionItem>,
    approvals: Vec<PendingApproval>,
    decisions: Vec<DecisionRecord>,
    missions: Vec<Mission>,
    claims: Vec<BoardEntry>,
    gates: HashMap<JobId, GateState>,
    details: HashMap<JobId, GateDetail>,
    plans: HashMap<String, PlanRecord>,
    stored: HashMap<String, Stored>,
    sessions: Vec<Session>,
    index: Option<Arc<Index>>,
    evidence_reads: Cell<usize>,
    metrics: Option<swarmux_domain::console::Metrics>,
}

impl Rig {
    fn new() -> Rig {
        let store = InMemoryStore::default();
        let sessions: Vec<Session> = [
            (1, "api-gateway"),
            (2, "web-checkout"),
            (3, "data-pipeline"),
            (4, "docs-site"),
            (5, "monorepo"),
        ]
        .iter()
        .map(|(n, name)| {
            Session::new(
                sid(*n),
                name,
                PathBuf::from(format!("/work/{name}")),
                "sonnet",
                0,
            )
            .unwrap()
        })
        .collect();
        for s in &sessions {
            SessionRepository::insert(&store, s).unwrap();
        }
        Rig {
            now: NOW,
            store,
            facts: Vec::new(),
            attention: Vec::new(),
            approvals: Vec::new(),
            decisions: Vec::new(),
            missions: Vec::new(),
            claims: Vec::new(),
            gates: HashMap::new(),
            details: HashMap::new(),
            plans: HashMap::new(),
            stored: HashMap::new(),
            sessions,
            index: None,
            evidence_reads: Cell::new(0),
            metrics: None,
        }
    }

    /// A job in the store (so its stream can be read) and as a fact.
    fn job(&mut self, session: u8, name: &str, status: JobStatus) -> JobId {
        let new = NewJob {
            session_id: sid(session),
            prompt_id: None,
            prompt: format!("job: {name}"),
            model: "sonnet".into(),
            priority: 0,
            fork: false,
            wants_review: false,
            review_of: None,
        };
        let id = JobRepository::insert(&self.store, &new, 1_000_000).unwrap();
        self.facts.push(fact(id.0, session, name, status));
        id
    }

    fn fact_mut(&mut self, id: JobId) -> &mut JobFact {
        self.facts.iter_mut().find(|f| f.job_id == id).unwrap()
    }

    fn event(&self, job: JobId, kind: &str, actor: &str, payload: &str, ts: Millis) {
        self.store
            .append_event(job, kind, actor, payload, ts)
            .unwrap();
    }

    fn sources(&self) -> Sources<'_> {
        let r = self;
        Sources {
            now: r.now,
            facts: &r.facts,
            attention: &r.attention,
            approvals: &r.approvals,
            decisions: &r.decisions,
            missions: &r.missions,
            claims: &r.claims,
            gates: &r.gates,
            queue: QueueStats {
                running: 1,
                queued: 1,
                max_concurrent: 4,
                free_ram: None,
                ram_low: false,
                deferred: 0,
                awaiting_reset: 0,
                on_battery: false,
                held: 0,
            },
            telemetry: None,
            stream: &r.store,
            plans: Box::new(move |slug: &str| r.plans.get(slug)),
            gate_detail: Box::new(move |job: JobId| r.details.get(&job)),
            contracts: Box::new(move |_: WorkspaceId| r.index.clone()),
            evidence: Box::new(move |slug: &str| {
                r.evidence_reads.set(r.evidence_reads.get() + 1);
                r.stored.get(slug).cloned()
            }),
            sessions: Box::new(move |id: SessionId| {
                r.sessions.iter().find(|s| s.id == id).cloned()
            }),
            metrics: r.metrics.as_ref(),
        }
    }

    fn snapshot(&self) -> ConsoleSnapshot {
        ConsoleService::new().snapshot(&self.sources()).clone()
    }
}

fn ask_call(
    job: JobId,
    request: u64,
    tool: &str,
    subject: &str,
    since: Millis,
) -> (AttentionItem, PendingApproval) {
    let call = HeldCall {
        request_id: request,
        job: Some(job),
        session: None,
        tool,
        subject,
        cwd: "/work/web-checkout",
    };
    let item = AttentionItem::approval(&call, since, NOW + 3_000_000);
    let pending = PendingApproval {
        request: HookRequest {
            request_id: request,
            job_id: Some(job),
            tool_name: tool.into(),
            subject: subject.into(),
            input: String::new(),
            cwd: "/work/web-checkout".into(),
            content: String::new(),
            ..HookRequest::default()
        },
        rule: None,
        secs_left: 3_000,
    };
    (item, pending)
}

fn decision(
    job: JobId,
    tool: &str,
    subject: &str,
    outcome: Outcome,
    rule: &str,
    ts: Millis,
) -> DecisionRecord {
    DecisionRecord {
        job_id: Some(job),
        tool: tool.into(),
        subject: subject.into(),
        outcome,
        rule: Some(rule.into()),
        latency_us: 40,
        ts,
        source: Source::Guard,
        confidence: None,
        extra: crate::DecisionExtra::default(),
    }
}

/// The demo's evening, in literals: a delivered mission, a job held at a
/// push and refused an edit of a protected test, a failed job, a queued
/// one, a verified one, a job nobody asks about, a plan to approve and a
/// task graph with a task that cannot merge.
fn evening() -> Rig {
    let mut r = Rig::new();

    // 1: the delivered mission, with its sealed bundle.
    let invoice = r.job(1, "api-gateway", JobStatus::Done);
    {
        let f = r.fact_mut(invoice);
        f.finished_at = Some(2_000_000);
        f.result = Some("Invoice totals round half up; the suite passes.".into());
        f.cost_usd = 1.84;
    }
    let mut m = mission("fix-invoice", MissionState::Delivered, 2_100_000);
    m.job = Some(invoice);
    m.spend = Spend {
        implement: 1.84,
        verify: 0.1,
        ..Spend::default()
    };
    r.missions.push(m);
    r.stored.insert("fix-invoice".into(), stored_bundle());
    r.attention.push(AttentionItem::result_ready(
        invoice,
        sid(1),
        "Fix invoice rounding",
        "Invoice totals round half up",
        2_000_000,
    ));

    // 2: a job held at a push, refused an edit before.
    let web = r.job(2, "web-checkout", JobStatus::Running);
    r.event(
        web,
        "text",
        "lead",
        "The retry test is in the way. I will edit its expectations.",
        1_600_000,
    );
    r.event(web, "usage", "lead", "in 1 out 2 cache 3/4", 1_600_100);
    r.event(
        web,
        "tool_use",
        "lead",
        "Edit tests/checkout_oracle.rs",
        1_600_500,
    );
    r.event(
        web,
        "tool_result",
        "lead",
        "ERROR denied by the Guard: protected path tests/** (gate)",
        1_600_600,
    );
    r.event(
        web,
        "text",
        "lead",
        "Pushing the branch with the test as it was.",
        1_700_000,
    );
    r.event(web, "text", "lead", "this one is after the call", 1_900_000);
    let (item, pending) = ask_call(web, 7, "Bash", "git push origin HEAD:main", 1_700_100);
    r.attention.push(item);
    r.approvals.push(pending);
    r.decisions = vec![
        decision(
            web,
            "Bash",
            "git push origin HEAD:main",
            Outcome::Ask,
            "git push asks",
            1_700_100,
        ),
        decision(
            web,
            "Edit",
            "tests/checkout_oracle.rs",
            Outcome::Deny,
            "protected path tests/** (gate)",
            1_600_550,
        ),
    ];
    r.claims.push({
        let mut c = BoardEntry::new("", web, BoardKind::Claim, "checkout", 1_000_000);
        c.areas = vec!["src/checkout/**".into()];
        c
    });

    // 3: a failed job whose gate gave up.
    let failed = r.job(3, "data-pipeline", JobStatus::Failed);
    {
        let f = r.fact_mut(failed);
        f.finished_at = Some(5_000);
        f.error = Some("claude exited with code 1".into());
    }
    r.attention.push(AttentionItem::job_failed(
        failed,
        sid(3),
        "Backfill the events table",
        "claude exited with code 1",
        5_000,
    ));
    r.gates.insert(
        failed,
        gate_state(
            GateOutcome::Escalate,
            &[("cargo test", false), ("clippy", true)],
            4_000,
        ),
    );
    r.details.insert(
        failed,
        GateDetail {
            runs: vec![
                run_of(
                    "cargo test",
                    false,
                    4_200,
                    Some("thread 'etl::counts' panicked at src/etl.rs:12"),
                ),
                run_of("clippy", true, 900, None),
            ],
            pins: vec!["changed tests/etl.rs".into()],
            tampering: Vec::new(),
            gaps: vec!["says the tests pass; no test ran".into()],
            base_suite: Some((false, "base suite: etl::counts failed".into())),
            ..GateDetail::default()
        },
    );

    // 4: a second job of the web session, queued behind the first.
    let queued = r.job(2, "web-checkout", JobStatus::Queued);
    {
        let f = r.fact_mut(queued);
        f.started_at = None;
        f.context = None;
    }
    r.claims.push({
        let mut c = BoardEntry::new("", queued, BoardKind::Claim, "submit", 1_000_000);
        c.areas = vec!["src/checkout/submit.ts".into()];
        c
    });

    // 5: finished, and nobody asks about it.
    let quiet = r.job(4, "docs-site", JobStatus::Done);
    r.fact_mut(quiet).finished_at = Some(3_000_000);
    r.attention.push(AttentionItem::result_ready(
        quiet,
        sid(4),
        "Regenerate the API reference",
        "Reference regenerated",
        3_000_000,
    ));

    // 6: finished and verified: ready to land.
    let ready = r.job(4, "docs-site", JobStatus::Done);
    r.fact_mut(ready).finished_at = Some(3_100_000);
    r.gates.insert(
        ready,
        gate_state(GateOutcome::Pass, &[("npm test", true)], 3_200_000),
    );
    r.details.insert(
        ready,
        GateDetail {
            runs: vec![run_of("npm test", true, 2_500, None)],
            ..GateDetail::default()
        },
    );

    // A plan waiting for approval.
    let mut plan_me = mission("plan-me", MissionState::Linted, 6_000);
    plan_me.session = Some(sid(5));
    r.missions.push(plan_me);
    r.plans.insert(
        "plan-me".into(),
        PlanRecord {
            status: PlanStatus::Proposed,
            plan: Some(Plan {
                summary: "split the checkout".into(),
                tasks: vec![task("a", &["src/a/**"], &[])],
            }),
            rounds: 1,
            ..PlanRecord::default()
        },
    );
    r.attention.push(AttentionItem::plan_ready(
        "plan-me",
        Some(sid(5)),
        "plan me",
        "1 task over 1 files: approve, change or cancel",
        6_000,
    ));

    // A task graph: t1 merged, t2 cannot merge, t3 waits for both.
    let t1 = r.job(5, "monorepo", JobStatus::Done);
    let t2 = r.job(5, "monorepo", JobStatus::Failed);
    let mut split = mission("split-app", MissionState::Running, 7_000);
    split.session = Some(sid(5));
    r.missions.push(split);
    let plan = Plan {
        summary: "api, db, then the ui".into(),
        tasks: vec![
            task("t1", &["src/api/**"], &[]),
            task("t2", &["src/db/**"], &[]),
            task("t3", &["src/ui/**"], &["t1", "t2"]),
        ],
    };
    let mut graph = GraphRun::new(&plan, "swarmux/split-app/integration", 1_000_000);
    graph.topology = Topology::Fanout;
    graph.tasks[0] = TaskRun {
        id: "t1".into(),
        state: TaskState::Merged,
        job: Some(t1),
        branch: "swarmux/split-app/t1".into(),
        ..TaskRun::default()
    };
    graph.tasks[1] = TaskRun {
        id: "t2".into(),
        state: TaskState::Failed,
        job: Some(t2),
        merge_blocked: true,
        note: "its merge conflicts in src/db/mod.rs".into(),
        ..TaskRun::default()
    };
    graph.merged = vec!["t1".into()];
    r.plans.insert(
        "split-app".into(),
        PlanRecord {
            status: PlanStatus::Approved,
            plan: Some(plan),
            rounds: 1,
            drift: Some(DriftReport {
                changed: 4,
                outside: vec!["scripts/x.sh".into()],
                outside_count: 1,
                tasks: 3,
                untouched: vec!["t3".into()],
                outside_share: 0.25,
                untouched_share: 0.33,
                threshold: 0.3,
                files: Vec::new(),
            }),
            graph: Some(graph),
            ..PlanRecord::default()
        },
    );
    r.attention.push(AttentionItem::merge_blocked(
        "split-app",
        "t2",
        (Some(sid(5)), Some(t2)),
        ("split app", "task t2: its merge conflicts in src/db/mod.rs"),
        7_000,
    ));
    r
}

fn task(id: &str, files: &[&str], after: &[&str]) -> PlanTask {
    PlanTask {
        id: id.into(),
        title: format!("task {id}"),
        files: files.iter().map(|s| s.to_string()).collect(),
        depends_on: after.iter().map(|s| s.to_string()).collect(),
        acceptance: "cargo test".into(),
    }
}

fn outcome<'a>(snap: &'a ConsoleSnapshot, id: &str) -> &'a OutcomeView {
    snap.groups
        .iter()
        .flat_map(|g| &g.outcomes)
        .find(|o| o.id == id)
        .unwrap_or_else(|| panic!("no outcome {id}"))
}

fn group(snap: &ConsoleSnapshot, kind: GroupKind) -> &GroupView {
    snap.groups.iter().find(|g| g.kind == kind).unwrap()
}

fn ids(g: &GroupView) -> Vec<&str> {
    g.outcomes.iter().map(|o| o.id.as_str()).collect()
}

// ---------------------------------------------------------------------
// Grouping and ranking
// ---------------------------------------------------------------------

#[test]
fn outcomes_land_in_the_right_groups_in_order() {
    let snap = evening().snapshot();
    assert_eq!(snap.schema, SCHEMA_VERSION);
    let kinds: Vec<GroupKind> = snap.groups.iter().map(|g| g.kind).collect();
    assert_eq!(
        kinds,
        [
            GroupKind::Decide,
            GroupKind::ReadyToLand,
            GroupKind::Running,
            GroupKind::Landed
        ]
    );
    // P1 first (the held push), then the P2s by age: the failed job
    // (5 000), the plan (6 000), the task that cannot merge (7 000).
    assert_eq!(
        ids(group(&snap, GroupKind::Decide)),
        ["job:2", "job:3", "mission:plan-me", "mission:split-app"]
    );
    assert_eq!(ids(group(&snap, GroupKind::ReadyToLand)), ["job:6"]);
    assert_eq!(ids(group(&snap, GroupKind::Running)), ["job:4"]);
    assert_eq!(
        ids(group(&snap, GroupKind::Landed)),
        ["mission:fix-invoice"]
    );
    let c = snap.header.counts;
    assert_eq!(
        (c.decide, c.ready_to_land, c.running, c.landed, c.idle),
        (4, 1, 1, 1, 1)
    );
    let push = outcome(&snap, "job:2");
    assert_eq!(push.priority, Some(PriorityKind::P1));
    assert_eq!(push.group, GroupKind::Decide);
    // The finished job nobody asks about is on no queue.
    assert!(snap.groups.iter().all(|g| !ids(g).contains(&"job:5")));
}

#[test]
fn a_mission_still_a_draft_is_not_an_outcome() {
    let mut r = Rig::new();
    r.missions.push(mission("later", MissionState::Draft, 1));
    r.missions
        .push(mission("refused", MissionState::Refused, 1));
    let snap = r.snapshot();
    assert!(snap.groups.iter().all(|g| g.total == 0));
    assert!(snap.deck.nodes.is_empty());
}

#[test]
fn the_landed_group_is_cut_to_its_newest_and_counted() {
    let mut r = Rig::new();
    for n in 0..5 {
        r.missions.push(mission(
            &format!("m{n}"),
            MissionState::Delivered,
            1_000 + n,
        ));
    }
    let snap = ConsoleService::new()
        .with_landed(2)
        .snapshot(&r.sources())
        .clone();
    let landed = group(&snap, GroupKind::Landed);
    assert_eq!(landed.total, 5);
    assert_eq!(ids(landed), ["mission:m4", "mission:m3"]);
    assert_eq!(snap.header.counts.landed, 5);
}

#[test]
fn an_escalated_mission_is_a_decision_even_without_an_ask() {
    let mut r = Rig::new();
    let mut m = mission("over-budget", MissionState::Escalated, 9_000);
    m.note = Some("spent $6.00 of $5.00".into());
    r.missions.push(m);
    let snap = r.snapshot();
    let o = outcome(&snap, "mission:over-budget");
    assert_eq!(o.group, GroupKind::Decide);
    assert_eq!(o.priority, Some(PriorityKind::P2));
    assert_eq!(
        o.contract.as_ref().unwrap().note.as_deref(),
        Some("spent $6.00 of $5.00")
    );
}

// ---------------------------------------------------------------------
// One outcome
// ---------------------------------------------------------------------

#[test]
fn a_delivered_mission_carries_its_contract_claim_and_sealed_evidence() {
    let snap = evening().snapshot();
    let o = outcome(&snap, "mission:fix-invoice");
    assert_eq!(o.kind, OutcomeKind::Mission);
    assert_eq!(o.lifecycle, Lifecycle::Delivered);
    assert_eq!(o.title, "fix invoice");
    let c = o.contract.as_ref().unwrap();
    assert_eq!(c.done_when, ["cargo test"]);
    assert_eq!(c.protected, ["tests/**"]);
    assert_eq!(c.work_in, ["src/**"]);
    assert_eq!(c.budget.usd, Some(5.0));
    assert_eq!(c.budget.on_exhaust, OnExhaust::Escalate);
    assert_eq!(o.jobs.len(), 1);
    assert_eq!(o.jobs[0].repo, "api-gateway");
    assert_eq!(o.jobs[0].context_use, Some(0.25));
    // The agent's claim is its final answer, as written.
    assert_eq!(
        o.claim.as_ref().unwrap().text,
        "Invoice totals round half up; the suite passes."
    );
    // Evidence: the bundle's hash, the ledger range, a signature that checks.
    let e = o.evidence.as_ref().unwrap();
    assert_eq!((e.seq_from, e.seq_to), (10, 42));
    assert_eq!(e.head_hash, "ab12cd");
    assert_eq!(e.seal_seq, Some(43));
    assert_eq!(e.signature, SignatureState::Valid);
    assert!(!e.anchored);
    let d = o.diff.as_ref().unwrap();
    assert_eq!((d.files, d.insertions, d.deletions), (3, 20, 4));
    // The checks come from the bundle once nothing ran this session.
    assert_eq!(o.checks.source, ChecksSource::Bundle);
    assert_eq!(o.checks.runs[0].name, "cargo test");
    assert_eq!(o.checks.runs[0].duration_ms, 4_200);
    assert_eq!(
        (o.checks.total, o.checks.passed, o.checks.failed),
        (1, 1, 0)
    );
    assert_eq!(o.checks.gate.unwrap().verdict, GateVerdict::Pass);
    // A mission worked in one folder lands nowhere.
    assert!(o.landing.is_none());
    assert_eq!(o.spend.implement_usd, 1.84);
}

#[test]
fn a_tampered_bundle_does_not_check_out() {
    let mut r = evening();
    r.stored.get_mut("fix-invoice").unwrap().bundle.head_sha = "tampered".into();
    let snap = r.snapshot();
    let e = outcome(&snap, "mission:fix-invoice")
        .evidence
        .clone()
        .unwrap();
    assert_eq!(e.signature, SignatureState::Invalid);
}

#[test]
fn the_checks_of_a_job_say_how_long_and_why_it_failed() {
    let snap = evening().snapshot();
    let o = outcome(&snap, "job:3");
    assert_eq!(o.lifecycle, Lifecycle::Failed);
    assert_eq!(o.checks.source, ChecksSource::Gate);
    let run = &o.checks.runs[0];
    assert_eq!(
        (run.name.as_str(), run.status),
        ("cargo test", CheckStatus::Failed)
    );
    assert_eq!(run.duration_ms, 4_200);
    assert_eq!(
        run.first_failure.as_deref(),
        Some("thread 'etl::counts' panicked at src/etl.rs:12")
    );
    assert_eq!(o.checks.runs[1].status, CheckStatus::Passed);
    assert_eq!(
        (o.checks.total, o.checks.passed, o.checks.failed),
        (2, 1, 1)
    );
    let g = o.checks.gate.unwrap();
    assert_eq!(
        (g.verdict, g.blocks, g.max_blocks),
        (GateVerdict::Escalate, 1, Some(3))
    );

    let stage = |key: StageKey| o.checks.stages.iter().find(|s| s.key == key).unwrap();
    let pins = stage(StageKey::ProtectedFiles);
    assert_eq!(pins.state, StageState::Failed);
    assert_eq!(pins.items, ["changed tests/etl.rs"]);
    assert_eq!(stage(StageKey::TestWeakening).state, StageState::Passed);
    assert_eq!(stage(StageKey::BaseSuite).state, StageState::Failed);
    assert_eq!(stage(StageKey::EvidenceGaps).state, StageState::Failed);
    assert_eq!(stage(StageKey::HiddenChecks).state, StageState::NotRun);
    assert_eq!(stage(StageKey::Review).state, StageState::NotRun);
    // Every stage is always answered, in order.
    let keys: Vec<StageKey> = o.checks.stages.iter().map(|s| s.key).collect();
    assert_eq!(
        keys,
        [
            StageKey::ProtectedFiles,
            StageKey::TestWeakening,
            StageKey::BaseSuite,
            StageKey::HiddenChecks,
            StageKey::EvidenceGaps,
            StageKey::Drift,
            StageKey::Review
        ]
    );
}

#[test]
fn hidden_checks_are_counted_and_a_reviewers_concerns_flag() {
    let mut r = evening();
    let id = JobId(6);
    r.gates.get_mut(&id).unwrap().hidden =
        Some(swarmux_domain::verifier::HiddenTally { ran: 4, passed: 3 });
    r.fact_mut(id).review = Some(ReviewState::Done {
        review: swarmux_domain::Review {
            verdict: swarmux_domain::ReviewVerdict::Concerns,
            concerns: vec!["no test for the retry cap".into()],
            diffs_checked: 2,
        },
        job: JobId(99),
        cost_usd: 0.1,
    });
    let snap = r.snapshot();
    let o = outcome(&snap, "job:6");
    let stage = |key: StageKey| o.checks.stages.iter().find(|s| s.key == key).unwrap();
    let hidden = stage(StageKey::HiddenChecks);
    assert_eq!((hidden.ran, hidden.passed), (Some(4), Some(3)));
    assert_eq!(hidden.state, StageState::Failed);
    assert_eq!(hidden.summary, "3 of 4 hidden checks passed");
    let review = stage(StageKey::Review);
    assert_eq!(review.state, StageState::Flagged);
    assert_eq!(review.items, ["no test for the retry cap"]);
}

#[test]
fn a_plan_comes_with_its_tasks_and_its_drift() {
    let snap = evening().snapshot();
    let o = outcome(&snap, "mission:split-app");
    let p = o.plan.as_ref().unwrap();
    assert_eq!(p.status, PlanStatus::Approved.into());
    assert_eq!(p.topology.as_deref(), Some("fanout"));
    let states: Vec<(&str, Option<super::view::TaskState>, Option<i64>)> = p
        .tasks
        .iter()
        .map(|t| (t.id.as_str(), t.state, t.job))
        .collect();
    assert_eq!(
        states,
        [
            ("t1", Some(super::view::TaskState::Merged), Some(7)),
            ("t2", Some(super::view::TaskState::Failed), Some(8)),
            ("t3", Some(super::view::TaskState::Waiting), None),
        ]
    );
    assert_eq!(p.tasks[0].files, ["src/api/**"]);
    assert_eq!(p.tasks[2].depends_on, ["t1", "t2"]);
    let d = p.drift.as_ref().unwrap();
    assert_eq!((d.changed, d.outside_count), (4, 1));
    assert_eq!(d.outside, ["scripts/x.sh"]);
    assert_eq!(d.untouched, ["t3"]);
    assert!(d.drifted);
    let drift_stage = o
        .checks
        .stages
        .iter()
        .find(|s| s.key == StageKey::Drift)
        .unwrap();
    assert_eq!(drift_stage.state, StageState::Flagged);
}

#[test]
fn a_workspace_mission_shows_what_landing_does_and_did() {
    let mut r = Rig::new();
    let mut m = mission("wide", MissionState::Running, 2_000);
    m.workspace = Some(workspace(1));
    m.contract.repos = vec!["api".into(), "mobile".into()];
    r.missions.push(m);
    let snap = r.snapshot();
    let o = outcome(&snap, "mission:wide");
    let l = o.landing.as_ref().unwrap();
    // The target is the mission's integration branch: never main.
    assert_eq!(l.into, "swarmux/wide/integration");
    assert_eq!(l.mode, LandingMode::Preview);
    let repos: Vec<&str> = l.repos.iter().map(|r| r.repo.as_str()).collect();
    assert_eq!(
        repos,
        ["api", "mobile"],
        "producers first, as the contract lists them"
    );
    assert!(l.repos.iter().all(|r| r.state == LandState::Waiting.into()));
    assert!(!l.complete);
    assert!(l.summary.contains("producers first, all or none"));

    // After the landing: what happened, repository by repository.
    let mut r = Rig::new();
    let mut m = mission("wide", MissionState::Delivered, 3_000);
    m.workspace = Some(workspace(1));
    let mut landing = Landing::new(
        "swarmux/wide/integration",
        &[
            ("api".to_string(), "swarmux/wide/1".to_string()),
            ("mobile".to_string(), "swarmux/wide/2".to_string()),
        ],
        2_900,
    );
    for repo in &mut landing.repos {
        repo.state = LandState::Landed;
        repo.commit = Some("c0ffee".into());
    }
    m.landing = Some(landing);
    r.missions.push(m);
    let snap = r.snapshot();
    let l = outcome(&snap, "mission:wide").landing.clone().unwrap();
    assert_eq!(l.mode, LandingMode::Result);
    assert!(l.complete);
    assert_eq!(l.at, Some(2_900));
    assert_eq!(l.repos[0].branch.as_deref(), Some("swarmux/wide/1"));
    assert_eq!(l.repos[1].commit.as_deref(), Some("c0ffee"));
}

fn workspace(n: u8) -> WorkspaceId {
    WorkspaceId::parse(&format!("0e000000-0000-4000-8000-{n:012}")).unwrap()
}

// ---------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------

fn action(o: &OutcomeView, kind: ActionKind) -> &ActionView {
    o.actions.iter().find(|a| a.kind == kind).unwrap()
}

#[test]
fn actions_follow_the_state_and_say_why_when_they_do_not() {
    let snap = evening().snapshot();
    // The held push: the call can be answered, the job stopped.
    let push = outcome(&snap, "job:2");
    assert_eq!(push.actions.len(), 10);
    assert!(action(push, ActionKind::AnswerHeldCall).enabled);
    assert_eq!(action(push, ActionKind::AnswerHeldCall).request, Some(7));
    assert!(action(push, ActionKind::Kill).enabled);
    assert!(!action(push, ActionKind::Retry).enabled);
    // The failed job: retry and resume.
    let failed = outcome(&snap, "job:3");
    assert!(action(failed, ActionKind::Retry).enabled);
    assert!(action(failed, ActionKind::Resume).enabled);
    assert!(!action(failed, ActionKind::Kill).enabled);
    assert!(action(failed, ActionKind::Kill).reason.is_some());
    // The plan: approve or reject.
    let plan = outcome(&snap, "mission:plan-me");
    assert!(action(plan, ActionKind::ApprovePlan).enabled);
    assert!(action(plan, ActionKind::RejectPlan).enabled);
    assert_eq!(
        action(plan, ActionKind::ApprovePlan).mission.as_deref(),
        Some("plan-me")
    );
    // The delivered mission: its evidence opens; nothing runs again.
    let done = outcome(&snap, "mission:fix-invoice");
    assert!(action(done, ActionKind::OpenEvidence).enabled);
    assert!(!action(done, ActionKind::Retry).enabled);
    // Send back: a held call is denied with the note, work that ended goes
    // back to its agent; delivered work and an unstarted plan cannot.
    assert!(action(push, ActionKind::SendBack).enabled);
    assert_eq!(action(push, ActionKind::SendBack).request, Some(7));
    assert!(action(failed, ActionKind::SendBack).enabled);
    assert_eq!(action(failed, ActionKind::SendBack).job, Some(3));
    let delivered = action(done, ActionKind::SendBack);
    assert!(!delivered.enabled);
    assert!(delivered.reason.as_deref().unwrap().contains("delivered"));
    assert!(!action(plan, ActionKind::SendBack).enabled);
    // Intervene: only a job that runs can be messaged, and a disabled
    // action always says why.
    let msg = action(push, ActionKind::Intervene);
    assert!(msg.enabled, "{:?}", msg.reason);
    assert_eq!(msg.job, Some(2));
    for ended in [failed, done] {
        let a = action(ended, ActionKind::Intervene);
        assert!(!a.enabled, "{}", ended.id);
        assert!(a.reason.as_deref().unwrap().contains("send it back"));
    }
    assert!(!action(plan, ActionKind::Intervene).enabled);
    assert!(action(plan, ActionKind::Intervene).reason.is_some());
    // Landing on request does not exist: never enabled, always explained.
    for o in snap.groups.iter().flat_map(|g| &g.outcomes) {
        let land = action(o, ActionKind::LandToIntegration);
        assert!(!land.enabled, "{}", o.id);
        assert!(land.reason.is_some(), "{}", o.id);
    }
}

#[test]
fn landing_is_offered_for_verified_workspace_work_only_when_the_user_lands() {
    let mut r = Rig::new();
    let job = r.job(1, "wide", JobStatus::Done);
    r.fact_mut(job).finished_at = Some(3_000_000);
    let mut m = mission("wide", MissionState::Verifying, 3_000_000);
    m.workspace = Some(workspace(1));
    m.job = Some(job);
    m.contract.repos = vec!["api".into(), "mobile".into()];
    r.missions.push(m);
    r.gates.insert(
        job,
        gate_state(GateOutcome::Pass, &[("cargo test", true)], 3_100_000),
    );

    // By hand: the verified mission waits in READY TO LAND, and landing
    // is enabled.
    let manual = ConsoleService::new()
        .with_landing(LandingPolicy::Manual)
        .snapshot(&r.sources())
        .clone();
    assert_eq!(
        ids(group(&manual, GroupKind::ReadyToLand)),
        ["mission:wide"]
    );
    let land = action(
        outcome(&manual, "mission:wide"),
        ActionKind::LandToIntegration,
    );
    assert!(land.enabled, "{:?}", land.reason);
    assert_eq!(land.mission.as_deref(), Some("wide"));
    let o = outcome(&manual, "mission:wide");
    assert_eq!(o.landing.as_ref().unwrap().mode, LandingMode::Preview);
    assert_eq!(o.landing.as_ref().unwrap().into, "swarmux/wide/integration");

    // Automatic (the default): swarmux lands it itself, so there is
    // nothing to ask for, and it says so.
    let auto = r.snapshot();
    let land = action(
        outcome(&auto, "mission:wide"),
        ActionKind::LandToIntegration,
    );
    assert!(!land.enabled);
    assert!(
        land.reason
            .as_deref()
            .unwrap()
            .contains("lands the work itself")
    );

    // Work the gate has not passed is not offered, either way.
    r.gates.clear();
    let waiting = ConsoleService::new()
        .with_landing(LandingPolicy::Manual)
        .snapshot(&r.sources())
        .clone();
    let land = action(
        outcome(&waiting, "mission:wide"),
        ActionKind::LandToIntegration,
    );
    assert!(!land.enabled);
    assert!(
        land.reason
            .as_deref()
            .unwrap()
            .contains("gate has not passed")
    );
}
// ---------------------------------------------------------------------
// Guard holds
// ---------------------------------------------------------------------

#[test]
fn a_held_call_quotes_the_agents_words_and_shows_its_countdown() {
    let snap = evening().snapshot();
    let held: Vec<&HoldView> = snap
        .holds
        .iter()
        .filter(|h| h.state == HoldState::Held)
        .collect();
    assert_eq!(held.len(), 1);
    let h = held[0];
    assert_eq!(h.id, "hold:7");
    assert_eq!(h.request, Some(7));
    assert_eq!(
        (h.tool.as_str(), h.target.as_str()),
        ("Bash", "git push origin HEAD:main")
    );
    assert_eq!(h.rule.as_deref(), Some("git push asks"));
    assert_eq!(h.decided_by, DecisionSource::Guard);
    assert_eq!(h.confidence, None);
    assert_eq!(h.priority, Some(PriorityKind::P1));
    assert!(
        h.destructive,
        "a push is irreversible: confirmed by naming the target"
    );
    assert_eq!(h.risk.unwrap().tier, RiskTier::Destructive);
    // The countdown: denied for lack of an answer at this moment.
    assert_eq!(h.deadline_at, Some(NOW + 3_000_000));
    assert_eq!(h.at, 1_700_100);
    assert_eq!(h.job, Some(2));
    assert_eq!(h.outcome.as_deref(), Some("job:2"));
    assert_eq!(h.session.as_deref(), Some("web-checkout"));
    // The agent's own words: the last text before the call, not the one after.
    let words = h.agent_words.as_ref().unwrap();
    assert_eq!(words.text, "Pushing the branch with the test as it was.");
    assert_eq!(words.at, 1_700_000);
    assert_eq!(words.steps_since, 0);
    assert!(!words.truncated);
    assert!(words.label.contains("agent's own words"));
    // The last steps up to the call, as the action stream shows them.
    let steps: Vec<(StepKind, &str)> = h.steps.iter().map(|s| (s.kind, s.text.as_str())).collect();
    assert_eq!(
        steps,
        [
            (
                StepKind::Text,
                "The retry test is in the way. I will edit its expectations."
            ),
            (StepKind::ToolCall, "Edit tests/checkout_oracle.rs"),
            (
                StepKind::ToolResult,
                "denied by the Guard: protected path tests/** (gate)"
            ),
            (
                StepKind::Text,
                "Pushing the branch with the test as it was."
            ),
        ]
    );
    assert_eq!(h.steps[2].ok, Some(false));
    assert_eq!(h.steps[0].ok, None);
}

#[test]
fn a_refused_call_is_listed_with_the_rule_and_the_words_before_it() {
    let snap = evening().snapshot();
    let denied: Vec<&HoldView> = snap
        .holds
        .iter()
        .filter(|h| h.state == HoldState::Denied)
        .collect();
    assert_eq!(denied.len(), 1);
    let d = denied[0];
    assert_eq!(d.id, "denied:2:1600550");
    assert_eq!(d.request, None);
    assert_eq!(d.deadline_at, None);
    assert_eq!(d.rule.as_deref(), Some("protected path tests/** (gate)"));
    assert_eq!(
        (d.tool.as_str(), d.target.as_str()),
        ("Edit", "tests/checkout_oracle.rs")
    );
    assert_eq!(d.outcome.as_deref(), Some("job:2"));
    assert_eq!(
        d.agent_words.as_ref().unwrap().text,
        "The retry test is in the way. I will edit its expectations."
    );
    // Held calls come first, then refusals.
    assert_eq!(snap.holds[0].state, HoldState::Held);
}

#[test]
fn the_agents_words_are_cut_and_say_so() {
    let r = evening();
    let web = JobId(2);
    let long = "word ".repeat(200);
    r.event(web, "text", "lead", &long, 1_700_050);
    let snap = r.snapshot();
    let w = snap.holds[0].agent_words.as_ref().unwrap();
    assert!(w.truncated);
    assert!(w.text.ends_with('…'));
    assert!(w.text.chars().count() <= 401);
}

#[test]
fn a_hold_without_a_stream_has_no_words_and_no_invention() {
    let mut r = Rig::new();
    let job = r.job(2, "web-checkout", JobStatus::Running);
    let (item, pending) = ask_call(job, 1, "Bash", "cargo check", 1_000);
    r.attention.push(item);
    r.approvals.push(pending);
    let snap = r.snapshot();
    let h = &snap.holds[0];
    assert!(h.agent_words.is_none());
    assert!(h.steps.is_empty());
    // Nothing logged the decision: the rule is unknown, not made up.
    assert_eq!(h.rule, None);
    assert_eq!(h.decided_by, DecisionSource::Guard);
}

#[test]
fn held_calls_are_ordered_by_priority_then_deadline() {
    let mut r = Rig::new();
    let a = r.job(2, "web-checkout", JobStatus::Running);
    let b = r.job(3, "data-pipeline", JobStatus::Running);
    let (ia, mut pa) = ask_call(a, 1, "Bash", "cargo check -p x", 1_000);
    let (ib, mut pb) = ask_call(b, 2, "Bash", "git push origin main", 2_000);
    pa.secs_left = 100;
    pb.secs_left = 900;
    r.attention.push(ia);
    r.attention.push(ib);
    r.approvals.push(pa);
    r.approvals.push(pb);
    let snap = r.snapshot();
    let order: Vec<&str> = snap.holds.iter().map(|h| h.id.as_str()).collect();
    // The push is P1; the routine check is P2 however soon it expires.
    assert_eq!(order, ["hold:2", "hold:1"]);
    let waiting = group(&snap, GroupKind::Decide);
    assert_eq!(ids(waiting), ["job:2", "job:1"]);
}

// ---------------------------------------------------------------------
// The Deck
// ---------------------------------------------------------------------

fn edge<'a>(deck: &'a DeckGraph, kind: EdgeKind, from: &str, to: &str) -> &'a DeckEdge {
    deck.edges
        .iter()
        .find(|e| e.kind == kind && e.from == from && e.to == to)
        .unwrap_or_else(|| panic!("no {kind:?} edge {from} -> {to}: {:?}", deck.edges))
}

fn node<'a>(deck: &'a DeckGraph, id: &str) -> &'a DeckNode {
    deck.nodes
        .iter()
        .find(|n| n.id == id)
        .unwrap_or_else(|| panic!("no node {id}"))
}

#[test]
fn a_task_graph_gives_a_card_per_task_and_dependency_edges() {
    let snap = evening().snapshot();
    let deck = &snap.deck;
    let t1 = node(deck, "task:split-app:t1");
    let t2 = node(deck, "task:split-app:t2");
    let t3 = node(deck, "task:split-app:t3");
    assert_eq!((t1.kind, t1.state), (NodeKind::Task, NodeState::Done));
    assert_eq!(t2.state, NodeState::Failed);
    assert_eq!(t3.state, NodeState::Waiting);
    assert_eq!(t1.outcome, "mission:split-app");
    assert_eq!((t1.job, t2.job, t3.job), (Some(7), Some(8), None));
    // The mission itself is not a card: its tasks are.
    assert!(!deck.nodes.iter().any(|n| n.id == "mission:split-app"));

    // t1 merged (met); t2 needs the user (blocked): the whole chain waits.
    assert_eq!(
        edge(deck, EdgeKind::TaskDependency, &t1.id, &t3.id).state,
        DepState::Met
    );
    assert_eq!(
        edge(deck, EdgeKind::TaskDependency, &t2.id, &t3.id).state,
        DepState::Blocked
    );
    // Producers sit before consumers.
    assert_eq!((t1.depth, t2.depth, t3.depth), (0, 0, 1));
    assert_eq!(deck.max_depth, 1);
    // The cards carry the group of their outcome.
    assert_eq!(t3.group, Some(GroupKind::Decide));
}

#[test]
fn claims_of_cards_in_one_repository_make_a_claim_edge() {
    let snap = evening().snapshot();
    let deck = &snap.deck;
    // Job 2 works in src/checkout/** and job 4 waits to touch a file in it.
    let e = edge(deck, EdgeKind::FileClaim, "job:2", "job:4");
    assert_eq!(e.state, DepState::Blocked);
    assert_eq!(e.detail, ["src/checkout/**"]);
    // Claims never order cards.
    assert_eq!(node(deck, "job:4").depth, 0);
}

#[test]
fn every_job_is_a_card_with_its_check_progress() {
    let snap = evening().snapshot();
    let deck = &snap.deck;
    let failed = node(deck, "job:3");
    assert_eq!(failed.state, NodeState::Failed);
    assert_eq!(
        (
            failed.checks.total,
            failed.checks.passed,
            failed.checks.failed
        ),
        (2, 1, 1)
    );
    assert_eq!(failed.repo.as_deref(), Some("data-pipeline"));
    assert_eq!(failed.group, Some(GroupKind::Decide));
    // The job nobody asks about is a card on no queue.
    let quiet = node(deck, "job:5");
    assert_eq!(quiet.group, None);
    assert_eq!(quiet.state, NodeState::Done);
    assert_eq!(node(deck, "job:6").state, NodeState::Ready);
    assert_eq!(node(deck, "job:4").state, NodeState::Queued);
    assert_eq!(node(deck, "mission:fix-invoice").state, NodeState::Done);
}

#[test]
fn contracts_link_the_cards_of_the_repositories_that_provide_and_use() {
    let mut r = Rig::new();
    let ws = workspace(7);
    for (slug, repo) in [("api-change", "api"), ("app-change", "app")] {
        let mut m = mission(slug, MissionState::Running, 1_000);
        m.workspace = Some(ws);
        m.contract.repos = vec![repo.into()];
        r.missions.push(m);
    }
    let provides = [Fact::provide(Symbol::table("orders"), 1, "id int")];
    let uses = [Fact::using(Symbol::table("orders"), 3)];
    let files = [
        FileFacts {
            repo: "api",
            path: "db/001.sql",
            hash: "h1",
            facts: &provides,
        },
        FileFacts {
            repo: "app",
            path: "lib/orders.dart",
            hash: "h2",
            facts: &uses,
        },
    ];
    r.index = Some(Arc::new(Index::build(&files)));
    let snap = r.snapshot();
    let e = edge(
        &snap.deck,
        EdgeKind::ContractLink,
        "mission:api-change",
        "mission:app-change",
    );
    assert_eq!(e.state, DepState::Pending);
    assert_eq!(e.detail, ["table orders"]);
    assert_eq!(node(&snap.deck, "mission:api-change").depth, 0);
    assert_eq!(node(&snap.deck, "mission:app-change").depth, 1);
}

// ---------------------------------------------------------------------
// The service: what it keeps, and when it rebuilds
// ---------------------------------------------------------------------

#[test]
fn the_snapshot_is_served_again_until_an_input_changes() {
    let mut r = evening();
    let mut svc = ConsoleService::new();
    let first = svc.snapshot(&r.sources()).clone();
    assert_eq!(svc.rebuilds(), 1);
    // The same inputs, asked for again and again (a frame, an idle tick).
    for _ in 0..5 {
        assert_eq!(svc.snapshot(&r.sources()), &first);
    }
    assert_eq!(svc.rebuilds(), 1);
    // The clock moving alone is not a change.
    r.now += 60_000;
    svc.snapshot(&r.sources());
    assert_eq!(svc.rebuilds(), 1);
    // A job finishing is.
    r.fact_mut(JobId(4)).status = JobStatus::Running;
    let next = svc.snapshot(&r.sources()).clone();
    assert_eq!(svc.rebuilds(), 2);
    assert_eq!(next.revision, 2);
    assert_ne!(next, first);
    // A gate verdict arriving is.
    r.gates
        .insert(JobId(4), gate_state(GateOutcome::Block, &[], 9_000_000));
    svc.snapshot(&r.sources());
    assert_eq!(svc.rebuilds(), 3);
}

#[test]
fn a_sealed_bundle_is_read_once_however_often_the_snapshot_is_rebuilt() {
    let mut r = evening();
    let mut svc = ConsoleService::new();
    svc.snapshot(&r.sources());
    assert_eq!(r.evidence_reads.get(), 1);
    for n in 0..4 {
        r.fact_mut(JobId(4)).cost_usd = 1.0 + f64::from(n);
        svc.snapshot(&r.sources());
    }
    assert_eq!(svc.rebuilds(), 5);
    assert_eq!(r.evidence_reads.get(), 1, "sealed evidence does not change");
}

#[test]
fn the_stream_is_read_as_a_bounded_tail() {
    let r = evening();
    let web = JobId(2);
    for n in 0..500 {
        r.event(web, "usage", "lead", "in 1 out 2 cache 3/4", 1_000_000 + n);
    }
    let tail = r
        .store
        .recent_events(web, 1_700_100, STREAM_WINDOW)
        .unwrap();
    assert_eq!(tail.len(), STREAM_WINDOW);
    assert!(tail.iter().all(|e| e.ts <= 1_700_100));
    // Even buried under usage events, the words before the call are found
    // when they are inside the window, and the steps stay bounded.
    let snap = r.snapshot();
    assert!(snap.holds[0].steps.len() <= 6);
}

// ---------------------------------------------------------------------
// The shape of the JSON
// ---------------------------------------------------------------------

/// Compares `value`'s JSON with the golden file `name`. Set
/// `SWARMUX_UPDATE_GOLDEN=1` to write the files (then review the diff).
fn golden<T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
    name: &str,
    value: &T,
) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/console/golden")
        .join(format!("{name}.json"));
    let text = serde_json::to_string_pretty(value).unwrap() + "\n";
    if std::env::var_os("SWARMUX_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| {
            panic!(
                "{}: {e} (SWARMUX_UPDATE_GOLDEN=1 writes it)",
                path.display()
            )
        })
        .replace("\r\n", "\n");
    assert_eq!(
        text, want,
        "the JSON shape of {name} changed: if that is on purpose, bump SCHEMA_VERSION when \
         it breaks clients and regenerate with SWARMUX_UPDATE_GOLDEN=1"
    );
    // And a client can read it back.
    let back: T = serde_json::from_str(&want).unwrap();
    assert_eq!(&back, value);
}

#[test]
fn the_snapshot_json_is_golden() {
    let snap = evening().snapshot();
    golden("snapshot", &snap);
}

#[test]
fn the_outcome_json_is_golden() {
    let snap = evening().snapshot();
    golden("outcome", outcome(&snap, "mission:fix-invoice"));
    golden("outcome_decide", outcome(&snap, "job:2"));
}

#[test]
fn the_hold_json_is_golden() {
    let snap = evening().snapshot();
    golden("hold_held", &snap.holds[0]);
    golden("hold_denied", &snap.holds[1]);
}

#[test]
fn the_deck_json_is_golden() {
    let snap = evening().snapshot();
    golden("deck", &snap.deck);
}

#[test]
fn the_header_json_is_golden() {
    let snap = evening().snapshot();
    golden("header", &snap.header);
}

#[test]
fn the_wire_words_are_spelled_out() {
    // Renaming a Rust variant must not move the wire: these are the words.
    let word = |v: &dyn erased::Json| v.json();
    assert_eq!(word(&GroupKind::ReadyToLand), "\"ready_to_land\"");
    assert_eq!(word(&AskKind::JobFailed), "\"job_failed\"");
    assert_eq!(
        word(&ActionKind::LandToIntegration),
        "\"land_to_integration\""
    );
    assert_eq!(word(&EdgeKind::TaskDependency), "\"task_dependency\"");
    assert_eq!(word(&PriorityKind::P1), "\"p1\"");
    assert_eq!(word(&DecisionSource::JevError), "\"jev_error\"");
    assert_eq!(word(&StageKey::BaseSuite), "\"base_suite\"");
    assert_eq!(word(&CheckStatus::TimedOut), "\"timed_out\"");
}

mod erased {
    pub trait Json {
        fn json(&self) -> String;
    }
    impl<T: serde::Serialize> Json for T {
        fn json(&self) -> String {
            serde_json::to_string(self).unwrap()
        }
    }
}

mod c3;
mod c5;

#[test]
fn hidden_checks_that_exist_and_did_not_run_say_so_with_why_never_a_bare_none() {
    let mut r = evening();
    // Job 3's gate: hidden checks exist and could not run.
    let failed = JobId(3);
    r.details.get_mut(&failed).unwrap().hidden_not_run =
        Some("hidden checks: not run (the test harness could not be put back)".into());
    let snap = r.snapshot();
    let o = outcome(&snap, "job:3");
    let hidden = o
        .checks
        .stages
        .iter()
        .find(|s| s.key == StageKey::HiddenChecks)
        .unwrap();
    assert_eq!(hidden.state, StageState::NotRun);
    assert_eq!(
        hidden.summary,
        "hidden checks: not run (the test harness could not be put back)"
    );
    assert_eq!((hidden.ran, hidden.passed), (None, None));

    // With nothing known, the older wording stands.
    let snap = evening().snapshot();
    let o = outcome(&snap, "job:3");
    let hidden = o
        .checks
        .stages
        .iter()
        .find(|s| s.key == StageKey::HiddenChecks)
        .unwrap();
    assert_eq!(hidden.summary, "no hidden checks ran");
}

#[test]
fn harness_files_the_job_changed_are_findings_on_the_weakening_stage() {
    let mut r = evening();
    let failed = JobId(3);
    r.details.get_mut(&failed).unwrap().substrate =
        vec!["conftest.py: harness file created (pytest runs it before every test)".into()];
    let snap = r.snapshot();
    let o = outcome(&snap, "job:3");
    let stage = o
        .checks
        .stages
        .iter()
        .find(|s| s.key == StageKey::TestWeakening)
        .unwrap();
    assert_eq!(stage.state, StageState::Failed);
    assert_eq!(
        stage.summary,
        "1 change(s) to tests or to the harness that runs them"
    );
    assert_eq!(
        stage.items,
        ["conftest.py: harness file created (pytest runs it before every test)"]
    );
}
