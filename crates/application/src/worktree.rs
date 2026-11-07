//! Worktrees: one git worktree and branch per job, with its own block of
//! ports, and the reclaiming of the ones nobody needs any more.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use swarmux_domain::ledger::{hex, sha256};
use swarmux_domain::{
    Checkpoint, Handoff, JobId, Millis, Repo, Session, SetMember, Workspace, WorktreeRecord,
    branch_name, slug,
};

use crate::checkpoint::HandoffBoard;
use crate::ledger::LedgerService;
use crate::ports::{Clock, JobRepository};
use crate::{AppError, AppResult, WorktreeSettings};

/// Environment variable naming the first port a job may use.
pub const ENV_PORT_BASE: &str = "SWARMUX_PORT_BASE";
/// Environment variable with how many consecutive ports the job owns.
pub const ENV_PORT_COUNT: &str = "SWARMUX_PORT_COUNT";
/// Environment variable with the worktree's folder.
pub const ENV_WORKTREE: &str = "SWARMUX_WORKTREE";
/// Environment variable with the job's branch.
pub const ENV_BRANCH: &str = "SWARMUX_BRANCH";
/// Environment variable with the job's number.
pub const ENV_JOB: &str = "SWARMUX_JOB";

/// A worktree to create.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddWorktree {
    pub repo: PathBuf,
    pub path: PathBuf,
    pub branch: String,
    /// Relative paths copied from the repository when they exist.
    pub copy: Vec<String>,
    /// The branch a new branch starts from; the repository's HEAD when `None`.
    pub base: Option<String>,
}

/// What creating a worktree did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Added {
    /// The branch did not exist and was created from the repository's HEAD.
    pub created_branch: bool,
    pub copied: Vec<String>,
}

/// What reclaiming a worktree did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Removed {
    /// Commit holding the unsaved files the folder still had.
    pub snapshot: Option<String>,
    /// The branch was merged and went with the worktree.
    pub branch_deleted: bool,
}

/// The git side of worktrees.
pub trait Worktrees {
    /// The top folder of the repository `dir` lies in.
    fn repository(&self, dir: &Path) -> AppResult<Option<PathBuf>>;
    /// Creates the worktree (on the existing branch when there is one) and
    /// copies the listed files into it.
    fn add(&self, req: &AddWorktree) -> AppResult<Added>;
    /// Commits every change in the worktree to its branch and returns the
    /// commit now at its head (the head itself when nothing changed).
    fn snapshot(&self, path: &Path, message: &str) -> AppResult<String>;
    /// Saves what is left in the folder, then removes it; the branch stays
    /// unless it is fully merged.
    fn remove(&self, repo: &Path, path: &Path, branch: &str) -> AppResult<Removed>;
    /// Whether the worktree's folder exists.
    fn present(&self, path: &Path) -> bool;
    /// Writes `handoff.json` where the worktree's own metadata lives, out
    /// of reach of `git add`.
    fn write_handoff(&self, path: &Path, handoff: &Handoff) -> AppResult<PathBuf>;
    /// Finishes the task folder of a worktree set once its worktrees are
    /// in it: marks it as one (so the gate takes it for the root) and
    /// copies the workspace root's own instructions (`CLAUDE.md`) that
    /// are not there yet. Returns what was copied.
    fn finish_task_dir(&self, _task_dir: &Path, _workspace_root: &Path) -> AppResult<Vec<String>> {
        Ok(Vec::new())
    }
}

/// Where worktrees and checkpoints are kept.
pub trait WorktreeStore {
    /// Stores a worktree. The members of one job's set share its port slot.
    fn insert(&self, record: &WorktreeRecord) -> AppResult<()>;
    /// The job's worktree: the first of its set, when it has one.
    fn get(&self, job: JobId) -> AppResult<Option<WorktreeRecord>>;
    /// Every worktree of the job, reclaimed or not, in set order.
    fn set(&self, job: JobId) -> AppResult<Vec<WorktreeRecord>> {
        Ok(self.get(job)?.into_iter().collect())
    }
    /// Worktrees whose folder has not been reclaimed, oldest first.
    fn active(&self) -> AppResult<Vec<WorktreeRecord>>;
    /// Hands a worktree (or all of a set) to the job that continues the
    /// work. False when `from` had none.
    fn rebind(&self, from: JobId, to: JobId) -> AppResult<bool>;
    /// Marks every worktree of the job reclaimed.
    fn mark_removed(&self, job: JobId, now: Millis) -> AppResult<()>;
    /// Stores the checkpoint and returns it with its id.
    fn insert_checkpoint(&self, checkpoint: &Checkpoint) -> AppResult<i64>;
    /// Oldest first.
    fn checkpoints(&self, job: JobId) -> AppResult<Vec<Checkpoint>>;
    /// The newest `limit`, newest first.
    fn recent_checkpoints(&self, limit: usize) -> AppResult<Vec<Checkpoint>>;
}

/// A job's worktree, ready to run in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    /// Where the job runs: the worktree, or the same subfolder of it that
    /// the session works in.
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub record: WorktreeRecord,
}

/// A workspace job's worktree set, ready to run in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedSet {
    /// The task folder the worktrees are laid out under.
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    /// One per repository the job touches, the shallowest first.
    pub records: Vec<WorktreeRecord>,
}

/// One row of `swarmux worktree list`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeView {
    pub record: WorktreeRecord,
    pub session_name: String,
    pub job_status: Option<String>,
    pub checkpoints: usize,
    /// The folder is gone.
    pub missing: bool,
    /// GC would reclaim it now.
    pub stale: bool,
}

/// What a GC pass did (or would do).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GcReport {
    pub reclaimed: Vec<WorktreeRecord>,
    /// Rows whose folder had already vanished.
    pub forgotten: Vec<WorktreeRecord>,
    pub kept: usize,
    pub errors: Vec<String>,
}

/// Creates, finds and reclaims job worktrees.
pub struct WorktreeService {
    git: Rc<dyn Worktrees>,
    store: Rc<dyn WorktreeStore>,
    jobs: Rc<dyn JobRepository>,
    clock: Rc<dyn Clock>,
    root: PathBuf,
    settings: WorktreeSettings,
    ledger: Option<Rc<LedgerService>>,
    board: Option<Arc<HandoffBoard>>,
    /// Branches new worktrees of queued jobs start from.
    bases: RefCell<HashMap<JobId, String>>,
}

impl WorktreeService {
    /// `root` is where worktrees are created unless the settings say otherwise.
    pub fn new(
        git: Rc<dyn Worktrees>,
        store: Rc<dyn WorktreeStore>,
        jobs: Rc<dyn JobRepository>,
        clock: Rc<dyn Clock>,
        root: PathBuf,
    ) -> WorktreeService {
        WorktreeService {
            git,
            store,
            jobs,
            clock,
            root,
            settings: WorktreeSettings::default(),
            ledger: None,
            board: None,
            bases: RefCell::new(HashMap::new()),
        }
    }

    pub fn with_settings(mut self, settings: &WorktreeSettings) -> WorktreeService {
        self.settings = settings.clone();
        self
    }

    /// Records created and reclaimed worktrees in the ledger.
    pub fn with_ledger(mut self, ledger: Rc<LedgerService>) -> WorktreeService {
        self.ledger = Some(ledger);
        self
    }

    /// Hands the latest handoff to resumed sessions.
    pub fn with_handoffs(mut self, board: Arc<HandoffBoard>) -> WorktreeService {
        self.board = Some(board);
        self
    }

    pub(crate) fn git(&self) -> &Rc<dyn Worktrees> {
        &self.git
    }

    pub(crate) fn store(&self) -> &Rc<dyn WorktreeStore> {
        &self.store
    }

    /// Whether jobs of `session` get a worktree: what the session says, else
    /// whether it is a workspace's (a set of them) or its folder is in a
    /// git repository.
    pub fn enabled_for(&self, session: &Session) -> bool {
        match session.worktree {
            Some(on) => on,
            None if session.workspace.is_some() => true,
            None => matches!(self.git.repository(&session.cwd), Ok(Some(_))),
        }
    }

    /// When `job` gets a new worktree, its branch starts from `base`.
    pub fn set_base(&self, job: JobId, base: &str) {
        self.bases.borrow_mut().insert(job, base.to_string());
    }

    /// The worktree of `job`, if it has one that was not reclaimed (the
    /// first of its set, when it has a set).
    pub fn record_for(&self, job: JobId) -> AppResult<Option<WorktreeRecord>> {
        Ok(self.store.get(job)?.filter(|r| r.removed_at.is_none()))
    }

    /// The worktrees of `job`'s set that were not reclaimed, the
    /// shallowest first: one for a job of a repository, none without.
    pub fn set_for(&self, job: JobId) -> AppResult<Vec<WorktreeRecord>> {
        let mut set: Vec<WorktreeRecord> = self
            .store
            .set(job)?
            .into_iter()
            .filter(|r| r.removed_at.is_none())
            .collect();
        set.sort_by_key(|r| r.member.as_ref().map_or(0, |m| m.seq));
        Ok(set)
    }

    /// Where `job` works: its task folder when it has a set, else its
    /// worktree.
    pub fn workdir_of(&self, job: JobId) -> AppResult<Option<PathBuf>> {
        Ok(self.record_for(job)?.map(|r| match r.member {
            Some(m) => m.task_dir,
            None => r.path,
        }))
    }

    /// The worktree set a workspace job runs in: the one it has (folders
    /// that vanished made again), or a new one with a worktree per
    /// repository in `repos`, laid out under a task folder the way they
    /// sit in the workspace. Files of the workspace root that are not in
    /// a repository are not copied, except its `CLAUDE.md`. `None` when
    /// worktrees are off for the session or `repos` is empty.
    pub fn prepare_set(
        &self,
        job: JobId,
        session: &Session,
        workspace: &Workspace,
        repos: &[Repo],
    ) -> AppResult<Option<PreparedSet>> {
        let have = self.set_for(job)?;
        if let Some(lead) = have.first() {
            for record in &have {
                if !self.git.present(&record.path) {
                    self.git.add(&AddWorktree {
                        repo: record.repo.clone(),
                        path: record.path.clone(),
                        branch: record.branch.clone(),
                        copy: self.settings.copy.clone(),
                        base: None,
                    })?;
                }
            }
            let task_dir = lead
                .member
                .as_ref()
                .map_or_else(|| lead.path.clone(), |m| m.task_dir.clone());
            self.git.finish_task_dir(&task_dir, &workspace.root)?;
            self.publish_handoff(job);
            return Ok(Some(self.ready_set(task_dir, have)));
        }
        self.bases.borrow_mut().remove(&job);
        if !self.enabled_for(session) || repos.is_empty() {
            return Ok(None);
        }
        let used: Vec<u32> = self.store.active()?.iter().map(|r| r.slot).collect();
        let plan = self.settings.ports;
        let slot = plan
            .lowest_free(&used)
            .ok_or_else(|| AppError::Workspace("no free port block left for a worktree".into()))?;
        let port_base = plan
            .port_base(slot)
            .ok_or_else(|| AppError::Workspace("port plan is out of range".into()))?;
        let task_dir = self.folder_for(&workspace.root, &session.name, job);
        let branch = branch_name(&session.name, job);
        // The shallowest first: a repository at the root holds the others.
        let mut order: Vec<&Repo> = repos.iter().collect();
        order.sort_by_key(|r| r.depth());
        let mut records = Vec::new();
        for (seq, repo) in order.into_iter().enumerate() {
            let path = repo.under(&task_dir);
            self.git.add(&AddWorktree {
                repo: repo.git_root.clone(),
                path: path.clone(),
                branch: branch.clone(),
                copy: self.settings.copy.clone(),
                base: None,
            })?;
            let record = WorktreeRecord {
                job,
                session: session.id,
                repo: repo.git_root.clone(),
                path,
                branch: branch.clone(),
                slot,
                port_base,
                port_count: plan.block,
                created_at: self.clock.now(),
                removed_at: None,
                member: Some(SetMember {
                    repo: repo.name.clone(),
                    task_dir: task_dir.clone(),
                    seq: u32::try_from(seq).unwrap_or(u32::MAX),
                }),
            };
            self.store.insert(&record)?;
            if let Some(ledger) = &self.ledger {
                ledger.record_worktree(&record, "created", None, self.clock.now())?;
            }
            records.push(record);
        }
        self.git.finish_task_dir(&task_dir, &workspace.root)?;
        Ok(Some(self.ready_set(task_dir, records)))
    }

    fn ready_set(&self, task_dir: PathBuf, records: Vec<WorktreeRecord>) -> PreparedSet {
        let mut env = Vec::new();
        if let Some(lead) = records.first() {
            env = vec![
                (ENV_PORT_BASE.to_string(), lead.port_base.to_string()),
                (ENV_PORT_COUNT.to_string(), lead.port_count.to_string()),
                (ENV_WORKTREE.to_string(), task_dir.display().to_string()),
                (ENV_BRANCH.to_string(), lead.branch.clone()),
                (ENV_JOB.to_string(), lead.job.to_string()),
            ];
        }
        PreparedSet {
            cwd: task_dir,
            env,
            records,
        }
    }

    /// The worktree `job` runs in: the one it has (made again if its folder
    /// vanished), or a new one. `None` when worktrees are off for the
    /// session or its folder is not in a repository.
    pub fn prepare(&self, job: JobId, session: &Session) -> AppResult<Option<Prepared>> {
        if let Some(record) = self.record_for(job)? {
            if !self.git.present(&record.path) {
                self.git.add(&AddWorktree {
                    repo: record.repo.clone(),
                    path: record.path.clone(),
                    branch: record.branch.clone(),
                    copy: self.settings.copy.clone(),
                    base: None,
                })?;
            }
            self.publish_handoff(job);
            return Ok(Some(self.ready(record, &session.cwd)));
        }
        if !self.enabled_for(session) {
            return Ok(None);
        }
        let Some(repo) = self.git.repository(&session.cwd)? else {
            return Ok(None);
        };
        let used: Vec<u32> = self.store.active()?.iter().map(|r| r.slot).collect();
        let plan = self.settings.ports;
        let slot = plan
            .lowest_free(&used)
            .ok_or_else(|| AppError::Workspace("no free port block left for a worktree".into()))?;
        let port_base = plan
            .port_base(slot)
            .ok_or_else(|| AppError::Workspace("port plan is out of range".into()))?;
        let path = self.folder_for(&repo, &session.name, job);
        let branch = branch_name(&session.name, job);
        self.git.add(&AddWorktree {
            repo: repo.clone(),
            path: path.clone(),
            branch: branch.clone(),
            copy: self.settings.copy.clone(),
            base: self.bases.borrow_mut().remove(&job),
        })?;
        let record = WorktreeRecord {
            job,
            session: session.id,
            repo,
            path,
            branch,
            slot,
            port_base,
            port_count: plan.block,
            created_at: self.clock.now(),
            removed_at: None,
            member: None,
        };
        self.store.insert(&record)?;
        if let Some(ledger) = &self.ledger {
            ledger.record_worktree(&record, "created", None, self.clock.now())?;
        }
        Ok(Some(self.ready(record, &session.cwd)))
    }

    fn ready(&self, record: WorktreeRecord, session_cwd: &Path) -> Prepared {
        let cwd = match session_cwd.strip_prefix(&record.repo) {
            Ok(rel) if !rel.as_os_str().is_empty() && record.path.join(rel).is_dir() => {
                record.path.join(rel)
            }
            _ => record.path.clone(),
        };
        let env = vec![
            (ENV_PORT_BASE.to_string(), record.port_base.to_string()),
            (ENV_PORT_COUNT.to_string(), record.port_count.to_string()),
            (ENV_WORKTREE.to_string(), record.path.display().to_string()),
            (ENV_BRANCH.to_string(), record.branch.clone()),
            (ENV_JOB.to_string(), record.job.to_string()),
        ];
        Prepared { cwd, env, record }
    }

    fn publish_handoff(&self, job: JobId) {
        let Some(board) = &self.board else { return };
        let latest = self
            .store
            .checkpoints(job)
            .ok()
            .and_then(|all| all.into_iter().rev().find_map(|c| c.handoff));
        if let Some(handoff) = latest {
            board.set(job, &handoff.briefing());
        }
    }

    fn folder_for(&self, repo: &Path, mission: &str, job: JobId) -> PathBuf {
        let root = self
            .settings
            .root
            .clone()
            .unwrap_or_else(|| self.root.clone());
        let repo_text = repo.to_string_lossy().to_lowercase().replace('\\', "/");
        let digest = hex(&sha256(repo_text.as_bytes()));
        // Short names: Windows paths get long fast.
        let mission = slug(mission);
        let mission: String = mission.chars().take(16).collect();
        root.join(&digest[..8]).join(format!("{mission}-{}", job.0))
    }

    /// Gives `from`'s worktree to `to`, which continues its work.
    pub fn rebind(&self, from: JobId, to: JobId) -> AppResult<bool> {
        let moved = self.store.rebind(from, to)?;
        if moved {
            self.publish_handoff(to);
        }
        Ok(moved)
    }

    /// Worktrees not yet reclaimed, with what the list shows about them.
    pub fn list(&self) -> AppResult<Vec<WorktreeView>> {
        let now = self.clock.now();
        self.store
            .active()?
            .into_iter()
            .map(|record| {
                let summary = self.jobs.summary(record.job)?;
                let checkpoints = self.store.checkpoints(record.job)?;
                let missing = !self.git.present(&record.path);
                let stale = self.is_stale(&record, now)?;
                Ok(WorktreeView {
                    session_name: summary
                        .as_ref()
                        .map(|s| s.session_name.clone())
                        .unwrap_or_default(),
                    job_status: summary.map(|s| s.status.as_str().to_string()),
                    checkpoints: checkpoints.len(),
                    missing,
                    stale,
                    record,
                })
            })
            .collect()
    }

    fn is_stale(&self, record: &WorktreeRecord, now: Millis) -> AppResult<bool> {
        let Some(summary) = self.jobs.summary(record.job)? else {
            // The job is gone from the database.
            return Ok(true);
        };
        if !summary.status.is_finished() {
            return Ok(false);
        }
        let ended = summary
            .started_at
            .unwrap_or(summary.created_at)
            .saturating_add(i64::try_from(summary.duration_ms.unwrap_or(0)).unwrap_or(0));
        let last_checkpoint = self
            .store
            .checkpoints(record.job)?
            .iter()
            .map(|c| c.at)
            .max()
            .unwrap_or(0);
        let idle_since = ended.max(last_checkpoint).max(record.created_at);
        let limit = i64::try_from(self.settings.stale_hours)
            .unwrap_or(i64::MAX / 3_600_000)
            .saturating_mul(3_600_000);
        Ok(now.saturating_sub(idle_since) >= limit)
    }

    /// Reclaims the worktrees of jobs that ended long enough ago, and
    /// forgets the ones whose folder is already gone. With `dry_run` it only
    /// reports. Unsaved files are committed to the job's branch first, so
    /// nothing is lost.
    pub fn gc(&self, dry_run: bool) -> AppResult<GcReport> {
        let now = self.clock.now();
        let mut report = GcReport::default();
        let mut active = self.store.active()?;
        // The worktrees of a set go together, the deepest first: one may
        // sit inside another.
        let mut first_seen: Vec<JobId> = Vec::new();
        for r in &active {
            if !first_seen.contains(&r.job) {
                first_seen.push(r.job);
            }
        }
        active.sort_by_key(|r| {
            let at = first_seen.iter().position(|j| *j == r.job).unwrap_or(0);
            (
                at,
                std::cmp::Reverse(r.member.as_ref().map_or(0, |m| m.seq)),
            )
        });
        let mut failed: Vec<JobId> = Vec::new();
        for record in active {
            let present = self.git.present(&record.path);
            if present && !self.is_stale(&record, now)? {
                report.kept += 1;
                continue;
            }
            if dry_run {
                if present {
                    report.reclaimed.push(record);
                } else {
                    report.forgotten.push(record);
                }
                continue;
            }
            // A set whose deeper worktree could not go keeps the rest.
            if failed.contains(&record.job) {
                report.kept += 1;
                continue;
            }
            // The set is marked reclaimed with its first worktree, the last
            // to go.
            let mark = record.member.as_ref().is_none_or(|m| m.seq == 0);
            if present {
                match self.git.remove(&record.repo, &record.path, &record.branch) {
                    Ok(removed) => {
                        self.finish_removal(&record, &removed, mark, now, &mut report);
                        report.reclaimed.push(record);
                    }
                    Err(e) => {
                        report
                            .errors
                            .push(format!("{}: {e}", record.path.display()));
                        report.kept += 1;
                        failed.push(record.job);
                    }
                }
            } else {
                self.finish_removal(&record, &Removed::default(), mark, now, &mut report);
                report.forgotten.push(record);
            }
        }
        Ok(report)
    }

    fn finish_removal(
        &self,
        record: &WorktreeRecord,
        removed: &Removed,
        mark: bool,
        now: Millis,
        report: &mut GcReport,
    ) {
        if mark && let Err(e) = self.store.mark_removed(record.job, now) {
            report.errors.push(e.to_string());
            return;
        }
        if let Some(ledger) = &self.ledger {
            let detail = removed.snapshot.as_deref();
            if let Err(e) = ledger.record_worktree(record, "reclaimed", detail, now) {
                report.errors.push(e.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests;
