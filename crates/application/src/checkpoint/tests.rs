use super::*;
use crate::hookbus::{HookNotice, SessionStartInfo};
use crate::testing::WorktreeRig;
use swarmux_domain::ledger::LedgerKind;

fn take(rig: &WorktreeRig, job: JobId, reason: CheckpointReason) -> Option<Checkpoint> {
    rig.checkpoints
        .take(
            job,
            "claude-1",
            reason,
            &Notes {
                state: "gate passed".into(),
                tried_and_failed: vec!["cargo test failed once".into()],
                next_step: String::new(),
            },
        )
        .unwrap()
}

#[test]
fn a_checkpoint_pairs_a_session_with_a_commit_and_a_ledger_entry() {
    let rig = WorktreeRig::new();
    let job = rig.job("make the answer right\nplease");
    rig.worktrees.prepare(job, &rig.session).unwrap().unwrap();
    let c = take(&rig, job, CheckpointReason::GatePass).expect("checkpoint");
    assert_eq!(c.session_id, "claude-1");
    assert_eq!(c.commit.len(), 40);
    assert_eq!(c.reason, CheckpointReason::GatePass);
    assert!(c.branch.starts_with("swarmux/demo-app/"));
    let handoff = c.handoff.as_ref().unwrap();
    assert!(handoff.goal.starts_with("make the answer right"));
    assert_eq!(handoff.tried_and_failed, ["cargo test failed once"]);
    assert!(!handoff.next_step.is_empty());

    let entries = rig.ledger.entries();
    let entry = entries
        .iter()
        .find(|e| e.draft.kind == LedgerKind::Checkpoint)
        .expect("ledger entry");
    assert_eq!(c.ledger_seq, Some(entry.seq));
    let p = &entry.draft.payload_json;
    assert!(p.contains(r#""reason":"gate_pass""#), "{p}");
    assert!(p.contains(&c.commit), "{p}");
    assert!(p.contains(r#""handoff":{"goal""#), "{p}");
    assert_eq!(rig.git.handoffs.borrow().len(), 1);
}

#[test]
fn checkpoints_are_listed_in_order_through_the_api() {
    let rig = WorktreeRig::new();
    let job = rig.job("x");
    rig.worktrees.prepare(job, &rig.session).unwrap().unwrap();
    let a = take(&rig, job, CheckpointReason::GatePass).unwrap();
    let b = take(&rig, job, CheckpointReason::PreCompact).unwrap();
    let all = rig.checkpoints.list(job).unwrap();
    assert_eq!(all.iter().map(|c| c.id).collect::<Vec<_>>(), [a.id, b.id]);
    assert_eq!(rig.checkpoints.latest(job).unwrap().unwrap().id, b.id);
    assert_eq!(rig.checkpoints.recent(1).unwrap()[0].id, b.id);
    assert!(rig.checkpoints.latest(JobId(999)).unwrap().is_none());
}

#[test]
fn no_worktree_means_no_checkpoint() {
    let rig = WorktreeRig::new();
    let job = rig.job("x");
    assert!(take(&rig, job, CheckpointReason::Manual).is_none());
    assert_eq!(rig.git.snapshots.get(), 0);
}

#[test]
fn the_handoff_reaches_the_session_that_starts_next() {
    let rig = WorktreeRig::new();
    let job = rig.job("x");
    rig.worktrees.prepare(job, &rig.session).unwrap().unwrap();
    let start = |source: &str| SessionStartInfo {
        job: Some(job),
        session: "claude-1".into(),
        source: source.into(),
    };
    assert!(rig.board.context(&start("compact")).is_none());
    take(&rig, job, CheckpointReason::PreCompact).unwrap();
    // A brand new session is not handed anything; a resumed or compacted
    // one is.
    assert!(rig.board.context(&start("startup")).is_none());
    let text = rig.board.context(&start("compact")).unwrap();
    assert!(text.contains("Goal: x"), "{text}");
    assert!(rig.board.context(&start("resume")).is_some());
}

#[test]
fn a_resumed_job_keeps_the_checkpoints_and_the_handoff() {
    let rig = WorktreeRig::new();
    let old = rig.job("x");
    rig.worktrees.prepare(old, &rig.session).unwrap().unwrap();
    take(&rig, old, CheckpointReason::GatePass).unwrap();
    let new = rig.job("x again");
    rig.worktrees.rebind(old, new).unwrap();
    assert!(rig.board.get(new).is_some());
    assert_eq!(rig.checkpoints.list(new).unwrap().len(), 1);
    assert!(rig.checkpoints.list(old).unwrap().is_empty());
}

fn pre_compact(job: JobId) -> HookNotice {
    HookNotice {
        event: swarmux_domain::HookEvent::PreCompact,
        job: Some(job),
        session: "claude-2".into(),
        cwd: "/repo".into(),
        tool: None,
        summary: "compacting".into(),
        detail: "auto".into(),
        input_sha256: String::new(),
        input_len: 0,
        raw: String::new(),
    }
}

#[test]
fn pre_compact_takes_a_checkpoint() {
    let rig = WorktreeRig::new();
    let job = rig.job("x");
    rig.worktrees.prepare(job, &rig.session).unwrap().unwrap();
    let feed = Rc::new(crate::testing::FakeHookFeed::default());
    let mut bus = crate::HookBus::new(
        feed.clone(),
        rig.ledger_service.clone(),
        rig.store.clone(),
        rig.clock.clone(),
    )
    .with_checkpoints(rig.checkpoints.clone());
    feed.push(pre_compact(job));
    assert!(bus.pump().is_empty());
    let c = rig.checkpoints.latest(job).unwrap().unwrap();
    assert_eq!(c.reason, CheckpointReason::PreCompact);
    assert_eq!(c.session_id, "claude-2");
    assert_eq!(bus.signals(job).unwrap().compactions, 1);
}
