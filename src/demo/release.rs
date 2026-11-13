//! The demo's second mission: a plan of three tasks built as a task graph,
//! kept as an earlier run would have kept it. The demo's jobs are scripted,
//! so the graph's state is written in advance (swarmux's own graph driver
//! needs worktrees and a merge queue, which the demo does not have); the
//! Deck reads it, and the jobs it points at, through the same services as
//! for a real run.

use swarmux_domain::mission::{Acceptance, Budget, Constraints, Contract, Mission, MissionState};
use swarmux_domain::plan::{Plan, PlanRecord, PlanStatus, PlanTask};
use swarmux_domain::taskgraph::{GraphRun, TaskRun, TaskState, Topology, integration_branch};
use swarmux_domain::{JobId, SessionId};

use super::script::START;

/// The mission's slug.
pub const SLUG: &str = "release-2-4";

/// The scripted jobs the tasks stand for, in the order the script starts
/// them: the data-pipeline backfill, the mobile-app push copy and the
/// docs-site reference.
const BACKFILL: JobId = JobId(1);
const PUSH_COPY: JobId = JobId(2);
const REFERENCE: JobId = JobId(3);

/// The mission, running, with its job still out (the graph holds the jobs).
pub fn mission(session: SessionId, root: &str) -> Mission {
    let mut m = Mission::new(
        SLUG,
        root,
        Contract {
            title: "Release 2.4".into(),
            goal: "Cut the 2.4 release: the events backfill, the push copy and the regenerated \
                   API reference land together."
                .into(),
            acceptance: Acceptance {
                checks: vec!["cargo test".into()],
                human_signoff: false,
            },
            constraints: Constraints {
                paths_rw: vec!["db/**".into(), "app/**".into(), "docs/**".into()],
                protected: vec!["tests/**".into()],
            },
            budget: Budget {
                usd: Some(60.0),
                wall_minutes: Some(180),
                ..Budget::default()
            },
            due: Some(START + 3 * 3_600_000),
            clarifications: Vec::new(),
            repos: Vec::new(),
        },
        START,
    );
    m.session = Some(session);
    m.state = MissionState::Running;
    m
}

fn task(id: &str, title: &str, files: &[&str], depends_on: &[&str], acceptance: &str) -> PlanTask {
    PlanTask {
        id: id.into(),
        title: title.into(),
        files: files.iter().map(|f| (*f).to_string()).collect(),
        depends_on: depends_on.iter().map(|d| (*d).to_string()).collect(),
        acceptance: acceptance.into(),
    }
}

fn run(id: &str, state: TaskState, job: JobId, note: &str) -> TaskRun {
    TaskRun {
        id: id.into(),
        state,
        job: Some(job),
        note: note.into(),
        started_at: Some(START),
        ..TaskRun::default()
    }
}

/// The approved plan: the reference waits for the other two.
pub fn plan() -> PlanRecord {
    let plan = Plan {
        summary: "Backfill the events table and update the push copy, then regenerate the API \
                  reference from the result."
            .into(),
        tasks: vec![
            task(
                "t1",
                "Backfill the events table",
                &["db/migrations/**"],
                &[],
                "the backfill migration applies and the events count matches",
            ),
            task(
                "t2",
                "Update the push notification copy",
                &["app/src/notifications/**"],
                &[],
                "the notification tests pass",
            ),
            task(
                "t3",
                "Regenerate the API reference",
                &["docs/api/**"],
                &["t1", "t2"],
                "the reference builds with no broken links",
            ),
        ],
    };
    let graph = GraphRun {
        topology: Topology::Fanout,
        integration: integration_branch(SLUG),
        tasks: vec![
            run("t1", TaskState::Merged, BACKFILL, "merged"),
            run("t2", TaskState::Merged, PUSH_COPY, "merged"),
            run("t3", TaskState::Running, REFERENCE, "running"),
        ],
        merged: vec!["t1".into(), "t2".into()],
        started_at: START,
        ..GraphRun::default()
    };
    PlanRecord {
        status: PlanStatus::Approved,
        plan: Some(plan),
        rounds: 1,
        cost_usd: 0.42,
        graph: Some(graph),
        ..PlanRecord::default()
    }
}
