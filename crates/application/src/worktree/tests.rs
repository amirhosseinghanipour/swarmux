use std::collections::HashSet;

use super::*;
use crate::ports::JobRepository;
use crate::testing::WorktreeRig;
use swarmux_domain::JobStatus;
use swarmux_domain::PortPlan;
use swarmux_domain::ledger::LedgerKind;

const HOUR: Millis = 3_600_000;

#[test]
fn a_job_gets_a_branch_a_folder_and_a_port_block() {
    let rig = WorktreeRig::new();
    let job = rig.job("do it");
    let p = rig
        .worktrees
        .prepare(job, &rig.session)
        .unwrap()
        .expect("a worktree");
    assert_eq!(p.record.branch, format!("swarmux/demo-app/{}", job.0));
    assert!(p.record.path.starts_with("/wt"), "{:?}", p.record.path);
    assert_eq!(p.cwd, p.record.path);
    assert_eq!(p.record.port_base, 20_000);
    let env: std::collections::HashMap<_, _> = p.env.iter().cloned().collect();
    assert_eq!(env[ENV_PORT_BASE], "20000");
    assert_eq!(env[ENV_PORT_COUNT], "20");
    assert_eq!(env[ENV_BRANCH], p.record.branch);
    assert_eq!(env[ENV_JOB], job.to_string());
    assert_eq!(rig.git.folders.borrow().len(), 1);
    let kinds: Vec<_> = rig.ledger.entries().iter().map(|e| e.draft.kind).collect();
    assert!(kinds.contains(&LedgerKind::Worktree));
}

#[test]
fn eight_worktrees_at_once_never_share_ports() {
    let rig = WorktreeRig::new();
    let mut bases = HashSet::new();
    let mut folders = HashSet::new();
    for n in 0..8 {
        let job = rig.job(&format!("task {n}"));
        let p = rig.worktrees.prepare(job, &rig.session).unwrap().unwrap();
        assert!(bases.insert(p.record.port_base));
        assert!(folders.insert(p.record.path.clone()));
    }
    let mut starts: Vec<u16> = bases.into_iter().collect();
    starts.sort_unstable();
    // Blocks are disjoint: no port of one is in another.
    assert!(starts.windows(2).all(|w| w[1] - w[0] >= 20), "{starts:?}");
}

#[test]
fn preparing_again_reuses_and_repairs_the_worktree() {
    let rig = WorktreeRig::new();
    let job = rig.job("x");
    let first = rig.worktrees.prepare(job, &rig.session).unwrap().unwrap();
    let again = rig.worktrees.prepare(job, &rig.session).unwrap().unwrap();
    assert_eq!(first.record, again.record);
    assert_eq!(rig.git.folders.borrow().len(), 1);
    // The folder vanished: made again on the same branch.
    rig.git.folders.borrow_mut().clear();
    rig.worktrees.prepare(job, &rig.session).unwrap().unwrap();
    assert_eq!(rig.git.folders.borrow().len(), 1);
}

#[test]
fn sessions_decide_and_folders_outside_a_repo_get_none() {
    let rig = WorktreeRig::new();
    let job = rig.job("x");
    let mut off = rig.session.clone();
    off.worktree = Some(false);
    assert!(rig.worktrees.prepare(job, &off).unwrap().is_none());
    assert!(rig.worktrees.enabled_for(&rig.session));

    let mut elsewhere = rig.session.clone();
    elsewhere.cwd = "/not-a-repo".into();
    assert!(!rig.worktrees.enabled_for(&elsewhere));
    assert!(rig.worktrees.prepare(job, &elsewhere).unwrap().is_none());
    // Forced on, but there is no repository to branch from.
    elsewhere.worktree = Some(true);
    assert!(rig.worktrees.prepare(job, &elsewhere).unwrap().is_none());
}

#[test]
fn running_out_of_port_blocks_is_an_error() {
    let settings = WorktreeSettings {
        ports: PortPlan {
            base: 30_000,
            block: 10,
            slots: 2,
        },
        ..WorktreeSettings::default()
    };
    let rig = WorktreeRig::with_settings(&settings);
    for _ in 0..2 {
        let job = rig.job("x");
        rig.worktrees.prepare(job, &rig.session).unwrap().unwrap();
    }
    let job = rig.job("x");
    assert!(rig.worktrees.prepare(job, &rig.session).is_err());
}

#[test]
fn rebinding_hands_the_worktree_to_the_new_job() {
    let rig = WorktreeRig::new();
    let old = rig.job("x");
    let first = rig.worktrees.prepare(old, &rig.session).unwrap().unwrap();
    let new = rig.job("x again");
    assert!(rig.worktrees.rebind(old, new).unwrap());
    let moved = rig.worktrees.prepare(new, &rig.session).unwrap().unwrap();
    assert_eq!(moved.record.path, first.record.path);
    assert_eq!(moved.record.port_base, first.record.port_base);
    assert!(rig.worktrees.record_for(old).unwrap().is_none());
    assert!(!rig.worktrees.rebind(old, new).unwrap());
}

fn finish(rig: &WorktreeRig, job: JobId, at: Millis) {
    rig.store.set_status(job, JobStatus::Done, at).unwrap();
}

#[test]
fn gc_reclaims_only_what_has_been_idle_long_enough() {
    let rig = WorktreeRig::new();
    let old = rig.job("old");
    let fresh = rig.job("fresh");
    let running = rig.job("running");
    for job in [old, fresh, running] {
        rig.worktrees.prepare(job, &rig.session).unwrap().unwrap();
    }
    finish(&rig, old, 0);
    finish(&rig, fresh, 0);
    rig.store
        .set_status(running, JobStatus::Running, 0)
        .unwrap();
    // Everything is older than the limit except `fresh`, which a
    // checkpoint just touched.
    rig.clock.advance(13 * HOUR);
    rig.checkpoints
        .take(
            fresh,
            "c1",
            swarmux_domain::CheckpointReason::Manual,
            &crate::Notes::default(),
        )
        .unwrap()
        .unwrap();

    let dry = rig.worktrees.gc(true).unwrap();
    assert_eq!(dry.reclaimed.len(), 1);
    assert!(rig.git.removed.borrow().is_empty());

    let report = rig.worktrees.gc(false).unwrap();
    assert_eq!(report.reclaimed.len(), 1);
    assert_eq!(report.reclaimed[0].job, old);
    assert_eq!(report.kept, 2);
    assert!(report.errors.is_empty());
    assert_eq!(rig.git.removed.borrow().len(), 1);
    assert!(rig.worktrees.record_for(old).unwrap().is_none());
    assert!(rig.worktrees.record_for(running).unwrap().is_some());
    // Its port block is free for the next job.
    let next = rig.job("next");
    let p = rig.worktrees.prepare(next, &rig.session).unwrap().unwrap();
    assert_eq!(p.record.slot, 0);
}

#[test]
fn gc_forgets_folders_that_vanished() {
    let rig = WorktreeRig::new();
    let job = rig.job("x");
    rig.worktrees.prepare(job, &rig.session).unwrap().unwrap();
    rig.git.folders.borrow_mut().clear();
    let report = rig.worktrees.gc(false).unwrap();
    assert_eq!(report.forgotten.len(), 1);
    assert!(rig.worktrees.record_for(job).unwrap().is_none());
    assert!(rig.worktrees.list().unwrap().is_empty());
}

#[test]
fn list_says_what_each_worktree_is_doing() {
    let rig = WorktreeRig::new();
    let job = rig.job("x");
    rig.worktrees.prepare(job, &rig.session).unwrap().unwrap();
    finish(&rig, job, 0);
    let rows = rig.worktrees.list().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].session_name, "Demo app");
    assert_eq!(rows[0].job_status.as_deref(), Some("done"));
    assert!(!rows[0].stale && !rows[0].missing);
    rig.clock.advance(25 * HOUR);
    assert!(rig.worktrees.list().unwrap()[0].stale);
}
