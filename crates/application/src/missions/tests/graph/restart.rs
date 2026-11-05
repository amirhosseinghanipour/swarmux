//! Task graphs across a restart of swarmux: where each task stood comes
//! back from the store, interrupted tasks carry on, and nothing finished
//! runs or merges twice.

use super::*;
use crate::missions::MAX_RESUMES;

/// Restart in the middle of a graph: merged tasks stay merged, the
/// interrupted ones carry on in their worktrees and sessions, a merge
/// whose news was lost is not made twice, and the mission delivers with
/// every task merged exactly once.
#[test]
fn a_graph_carries_on_after_a_restart_and_merges_each_task_once() {
    let mut g = rig(4);
    let slug = g.approved(decomposable());
    g.tick();
    let (j1, j2, j3, j4) = (
        g.job_of(&slug, "t1"),
        g.job_of(&slug, "t2"),
        g.job_of(&slug, "t3"),
        g.job_of(&slug, "t4"),
    );
    let (b1, b4) = (g.branch_of(j1), g.branch_of(j4));
    // t3 finished and merged before the stop.
    g.finish(j3, 0.5);
    g.tick();
    assert_eq!(g.state(&slug, "t3"), TaskState::Merged);
    // t4 merged too, but swarmux stopped before the run was saved.
    let saved = g.plans_store.list().unwrap();
    g.finish(j4, 0.5);
    g.tick();
    assert_eq!(g.state(&slug, "t4"), TaskState::Merged);
    for (s, r) in saved {
        g.plans_store.save(&s, &r, 0).unwrap();
    }
    // t1's agent was some steps in; t2's never began its session.
    g.begins(j1);

    let report = g.restart();
    assert!(report.changed);
    assert_eq!(g.state(&slug, "t3"), TaskState::Merged);
    let n1 = g.job_of(&slug, "t1");
    let n2 = g.job_of(&slug, "t2");
    assert!(n1 != j1 && n2 != j2);
    let r1 = g.runner.request(g.index_of(n1));
    let r2 = g.runner.request(g.index_of(n2));
    // t1 picks up its own conversation in its own worktree: at most the
    // step that was under way is lost. t2 starts its task again.
    assert!(r1.resume, "{r1:?}");
    assert_eq!(g.branch_of(n1), b1);
    assert!(!r2.resume);
    assert!(r2.prompt.contains("Your task: t2"), "{}", r2.prompt);
    assert_eq!(g.run(&slug).task("t1").unwrap().resumes, 1);
    assert_eq!(g.ledger("resumed").len(), 2);
    // Claims and task gates came back with them.
    let held: Vec<JobId> = g.claims.held().into_iter().map(|(j, _)| j).collect();
    assert!(held.contains(&n1) && held.contains(&n2), "{held:?}");
    assert!(g.gates.assigned.borrow().iter().any(|(j, _)| *j == n1));
    // Nothing that had finished ran again.
    assert_eq!(g.runner.started(), 2);

    g.tick();
    assert_eq!(g.state(&slug, "t4"), TaskState::Merged);
    g.finish(n1, 0.5);
    g.finish(n2, 0.5);
    g.tick();
    let t5 = g.job_of(&slug, "t5");
    g.finish(t5, 0.5);
    g.tick();
    g.pass_final(&slug);
    g.tick();
    assert_eq!(g.svc.get(&slug).unwrap().state, MissionState::Delivered);
    let merged = g.merge.merged.borrow().clone();
    assert_eq!(merged.len(), 5, "{merged:?}");
    assert_eq!(merged.iter().filter(|b| **b == b4).count(), 1);
    let order = g.run(&slug).merged;
    let mut unique = order.clone();
    unique.sort();
    unique.dedup();
    assert_eq!((order.len(), unique.len()), (5, 5), "{order:?}");
}

/// Restarting twice mid-graph, and once more when it is done, never runs
/// a finished task again nor merges anything twice.
#[test]
fn restarting_twice_runs_and_merges_nothing_twice() {
    let mut g = rig(4);
    let slug = g.approved(decomposable());
    g.tick();
    for id in ["t1", "t2"] {
        let job = g.job_of(&slug, id);
        g.finish(job, 0.5);
    }
    g.tick();
    for id in ["t3", "t4"] {
        let job = g.job_of(&slug, id);
        g.begins(job);
    }
    g.restart();
    for id in ["t3", "t4"] {
        let job = g.job_of(&slug, id);
        g.begins(job);
    }
    g.restart();
    // Two restarts, two resumes; the merged tasks are left alone.
    assert_eq!(g.run(&slug).task("t3").unwrap().resumes, 2);
    assert_eq!(g.runner.started(), 2);
    for id in ["t3", "t4"] {
        let job = g.job_of(&slug, id);
        g.finish(job, 0.5);
    }
    g.tick();
    let t5 = g.job_of(&slug, "t5");
    g.finish(t5, 0.5);
    g.tick();
    g.pass_final(&slug);
    g.tick();
    assert_eq!(g.svc.get(&slug).unwrap().state, MissionState::Delivered);
    g.restart();
    g.tick();
    assert_eq!(g.runner.started(), 0, "a delivered mission starts nothing");
    assert_eq!(g.merge.merged.borrow().len(), 5);
    assert_eq!(g.ledger("merged").len(), 5);
}

/// A task interrupted more than [`MAX_RESUMES`] times goes to the user
/// rather than round again.
#[test]
fn a_task_interrupted_too_often_goes_to_the_user() {
    let mut g = rig(4);
    let slug = g.approved(decomposable());
    g.tick();
    for _ in 0..=MAX_RESUMES {
        let job = g.job_of(&slug, "t1");
        g.begins(job);
        g.restart();
    }
    g.tick();
    assert_eq!(g.state(&slug, "t1"), TaskState::Failed);
    assert_eq!(g.run(&slug).task("t1").unwrap().resumes, MAX_RESUMES);
}

/// A graph whose integrated result was being verified is checked again.
#[test]
fn a_graph_being_verified_is_checked_again_after_a_restart() {
    let mut g = rig(4);
    let slug = g.approved(decomposable());
    for id in ["t1", "t2", "t3", "t4"] {
        let job = g.job_of(&slug, id);
        g.finish(job, 0.1);
    }
    g.tick();
    let t5 = g.job_of(&slug, "t5");
    g.finish(t5, 0.1);
    g.tick();
    assert_eq!(g.svc.get(&slug).unwrap().state, MissionState::Verifying);
    g.restart();
    assert_eq!(g.gates.verified.borrow().last().unwrap().0, t5);
    assert!(!g.gates.assigned.borrow().is_empty());
    g.pass_final(&slug);
    g.tick();
    assert_eq!(g.svc.get(&slug).unwrap().state, MissionState::Delivered);
}
