//! Jobs of workspace sessions: where they run, which repositories they
//! see, their worktree sets and checkpoints.

use std::path::Path;

use super::*;
use crate::Notes;
use crate::testing::{FakeRunner, InMemoryWorkspaces, WorktreeRig};
use swarmux_domain::{CheckpointReason, Repo, Workspace, WorkspaceId};

struct Rig {
    wt: WorktreeRig,
    runner: Rc<FakeRunner>,
    svc: JobService,
    workspace: Workspace,
    session: Session,
}

fn repo(name: &str, path: &str) -> Repo {
    Repo {
        name: name.into(),
        path: path.into(),
        git_root: Path::new("/p").join(path),
        default_branch: Some("main".into()),
        stacks: Vec::new(),
        commands: Vec::new(),
    }
}

fn rig(worktrees: bool) -> Rig {
    let wt = WorktreeRig::new();
    let runner = Rc::new(FakeRunner::default());
    let mut workspace = Workspace::new(WorkspaceId::random(), "Product", "/p".into(), 0).unwrap();
    workspace.set_repos(
        vec![
            repo("app", "app"),
            repo("backend", "backend"),
            repo("admin", "web/admin"),
        ],
        0,
    );
    let store = Rc::new(InMemoryWorkspaces::default());
    store.save(&workspace).unwrap();
    let mut session =
        Session::new(SessionId::random(), "Product", "/p".into(), "sonnet", 0).unwrap();
    session.workspace = Some(workspace.id);
    SessionRepository::insert(&*wt.store, &session).unwrap();
    let mut svc = JobService::new(
        wt.store.clone(),
        wt.store.clone(),
        runner.clone(),
        wt.clock.clone(),
    )
    .with_limits(Limits {
        max_concurrent: 8,
        min_free_ram: 0,
    })
    .with_workspaces(store);
    if worktrees {
        svc = svc.with_worktrees(wt.worktrees.clone());
    }
    Rig {
        wt,
        runner,
        svc,
        workspace,
        session,
    }
}

#[test]
fn without_worktrees_a_workspace_job_runs_at_the_root_with_its_repos_added() {
    let mut r = rig(false);
    r.svc
        .dispatch_scoped(
            r.session.id,
            "wire the api",
            None,
            &["backend".into(), "app".into()],
        )
        .unwrap();
    let req = r.runner.request(0);
    assert_eq!(req.cwd, Path::new("/p"));
    assert_eq!(
        req.add_dirs,
        [Path::new("/p").join("backend"), Path::new("/p").join("app")]
    );
    r.runner.emit(
        0,
        RunnerMsg::Exited {
            code: Some(0),
            stderr_tail: String::new(),
        },
    );
    r.svc.pump();
    // Naming none means every repository.
    r.svc.dispatch(r.session.id, "look around").unwrap();
    assert_eq!(r.runner.request(1).add_dirs.len(), 3);
}

#[test]
fn a_workspace_job_gets_a_worktree_set_for_the_repos_it_touches() {
    let mut r = rig(true);
    let job = r
        .svc
        .dispatch_scoped(
            r.session.id,
            "wire the api",
            None,
            &["backend".into(), "admin".into()],
        )
        .unwrap();
    let req = r.runner.request(0);
    let set = r.wt.worktrees.set_for(job).unwrap();
    assert_eq!(set.len(), 2, "app is not touched: no worktree for it");
    let task = set[0].member.as_ref().unwrap().task_dir.clone();
    assert_eq!(req.cwd, task, "the job works in the task folder");
    // Laid out like the workspace, the shallowest first.
    assert_eq!(set[0].path, task.join("backend"));
    assert_eq!(set[1].path, task.join("web").join("admin"));
    assert_eq!(set[0].repo, Path::new("/p").join("backend"));
    assert_eq!(set[0].repo_name(), Some("backend"));
    assert_eq!(req.add_dirs, [set[0].path.clone(), set[1].path.clone()]);
    assert_eq!(set[0].slot, set[1].slot, "one port block for the set");
    assert_eq!(set[0].branch, set[1].branch);
    let env = |k: &str| req.env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    assert_eq!(
        env(crate::worktree::ENV_WORKTREE),
        Some(task.display().to_string())
    );
    assert_eq!(r.wt.worktrees.workdir_of(job).unwrap(), Some(task));
    // Only the set's repositories had worktrees made.
    assert_eq!(r.wt.git.folders.borrow().len(), 2);
}

#[test]
fn a_resumed_workspace_job_keeps_its_set_and_scope() {
    let mut r = rig(true);
    let job = r
        .svc
        .dispatch_scoped(r.session.id, "wire the api", None, &["app".into()])
        .unwrap();
    let before = r.wt.worktrees.set_for(job).unwrap();
    r.runner.emit(
        0,
        RunnerMsg::Exited {
            code: Some(1),
            stderr_tail: String::new(),
        },
    );
    r.svc.pump();
    let again = r.svc.resume(job).unwrap();
    assert_eq!(r.svc.scope(again), ["app"]);
    let after = r.wt.worktrees.set_for(again).unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].path, before[0].path);
    assert_eq!(r.runner.request(1).cwd, r.runner.request(0).cwd);
}

#[test]
fn unknown_repos_fail_the_job_with_the_reason() {
    let mut r = rig(true);
    let job = r
        .svc
        .dispatch_scoped(r.session.id, "x", None, &["web".into()])
        .unwrap();
    assert_eq!(r.runner.started(), 0);
    let p = r.svc.progress(job).unwrap();
    assert_eq!(p.status, JobStatus::Failed);
}

#[test]
fn checkpoints_commit_every_repo_of_the_set() {
    let mut r = rig(true);
    let job = r
        .svc
        .dispatch_scoped(r.session.id, "x", None, &["app".into(), "backend".into()])
        .unwrap();
    let lead =
        r.wt.checkpoints
            .take(job, "", CheckpointReason::Manual, &Notes::default())
            .unwrap()
            .unwrap();
    let all = r.wt.checkpoints.list(job).unwrap();
    assert_eq!(all.len(), 2, "one per repository");
    assert_eq!(r.wt.git.snapshots.get(), 2);
    assert_eq!(all.last().unwrap().worktree, lead.worktree);
    assert_eq!(
        r.wt.checkpoints.latest(job).unwrap().unwrap().worktree,
        r.wt.worktrees.record_for(job).unwrap().unwrap().path
    );
    let _ = &r.workspace;
}

#[test]
fn a_set_is_reclaimed_whole_deepest_first() {
    let mut r = rig(true);
    let job = r.svc.dispatch_scoped(r.session.id, "x", None, &[]).unwrap();
    r.runner.emit(
        0,
        RunnerMsg::Exited {
            code: Some(0),
            stderr_tail: String::new(),
        },
    );
    r.svc.pump();
    r.wt.clock.advance(13 * 3_600_000);
    let gc = r.wt.worktrees.gc(false).unwrap();
    assert_eq!(gc.reclaimed.len(), 3, "{gc:?}");
    assert_eq!(
        gc.reclaimed[0].repo_name(),
        Some("admin"),
        "the deepest goes first"
    );
    assert!(r.wt.worktrees.set_for(job).unwrap().is_empty());
}
