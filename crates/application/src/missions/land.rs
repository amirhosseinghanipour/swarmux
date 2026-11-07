//! Workspace missions: a mission of a workspace runs one job across the
//! repositories it names, each in a worktree of the job's set. Once the
//! gate passed it lands: every repository's branch is merged into the
//! mission's branch in that repository (`swarmux/<mission>/integration`,
//! as for task graphs; the user's own branches are never written), in the
//! order the mission names them, producers first. When one repository
//! can't take its merge, those merged before it are put back and the
//! mission goes to the user: a workspace change never half lands.

use std::rc::Rc;

use swarmux_domain::mission::{
    Compiled, Finding, LandState, Landing, Mission, Severity, compile_in, lint_repos,
};
use swarmux_domain::taskgraph::integration_branch;
use swarmux_domain::{AttentionItem, JobId, Workspace, WorktreeRecord};

use super::{MergeOutcome, MissionService};
use crate::workspace::WorkspaceStore;
use crate::{AppError, AppResult};

impl MissionService {
    /// Looks up the workspaces of workspace missions, to check the
    /// repositories they name and to land them.
    pub fn with_workspaces(mut self, workspaces: Rc<dyn WorkspaceStore>) -> MissionService {
        self.workspaces = Some(workspaces);
        self
    }

    /// The mission's workspace, while it has one swarmux still knows.
    pub(super) fn workspace_of(&self, m: &Mission) -> AppResult<Option<Workspace>> {
        match (m.workspace, &self.workspaces) {
            (Some(id), Some(store)) => store.get(id),
            _ => Ok(None),
        }
    }

    /// What the mission names that its workspace does not have.
    pub(super) fn repo_findings(&self, m: &Mission) -> AppResult<Vec<Finding>> {
        if m.workspace.is_none() {
            return Ok(lint_repos(&m.contract, None));
        }
        let known = self
            .workspace_of(m)?
            .map(|w| w.repo_names())
            .unwrap_or_default();
        Ok(lint_repos(&m.contract, Some(&known)))
    }

    /// The mission compiled, with a workspace mission's `repo:` checks
    /// run in their repositories' folders.
    pub(super) fn compiled(&self, i: usize) -> Result<Compiled, Vec<Finding>> {
        let m = &self.missions[i];
        let workspace = self.workspace_of(m).ok().flatten();
        let blocking: Vec<Finding> = self
            .repo_findings(m)
            .unwrap_or_default()
            .into_iter()
            .filter(|f| f.severity == Severity::Blocking)
            .collect();
        if !blocking.is_empty() {
            return Err(blocking);
        }
        match workspace {
            Some(w) => compile_in(&m.contract, &|name: &str| {
                w.repo(name).map(|r| r.path.clone())
            }),
            None => compile_in(&m.contract, &|d: &str| Some(d.to_string())),
        }
    }

    /// Lands the repositories of a workspace mission whose gate passed.
    /// `None` when all of them landed (or the job worked without
    /// worktrees, so there is nothing to land); why not otherwise.
    pub(super) fn land(&mut self, i: usize, job: JobId) -> AppResult<Option<String>> {
        let (Some(merge), Some(worktrees)) =
            (self.graphs.merge.clone(), self.graphs.worktrees.clone())
        else {
            return Ok(None);
        };
        let set = worktrees.set_for(job)?;
        if set.iter().all(|r| r.member.is_none()) {
            return Ok(None);
        }
        let slug = self.missions[i].slug.clone();
        let order = self.landing_order(i, &set)?;
        let into = integration_branch(&slug);
        let names: Vec<(String, String)> = order
            .iter()
            .map(|r| {
                (
                    r.repo_name().unwrap_or_default().to_string(),
                    r.branch.clone(),
                )
            })
            .collect();
        let mut landing = Landing::new(&into, &names, self.clock.now());
        // What each landed repository's branch was before: where it goes
        // back to.
        let mut landed: Vec<(String, std::path::PathBuf, String)> = Vec::new();
        for record in &order {
            let repo = record.repo_name().unwrap_or_default().to_string();
            let step = (|| -> AppResult<Result<Option<String>, String>> {
                worktrees
                    .git()
                    .snapshot(&record.path, &format!("swarmux: {repo} for mission {slug}"))?;
                let workdir = merge.open(&record.repo, &into)?;
                if merge.contains(&workdir, &record.branch)? {
                    return Ok(Ok(None));
                }
                let before = merge.head(&workdir)?;
                let message = format!("swarmux: land {repo} of mission {slug}");
                match merge.merge(&workdir, &record.branch, &message)? {
                    MergeOutcome::Merged { commit } => {
                        landed.push((repo.clone(), workdir, before));
                        Ok(Ok(Some(commit)))
                    }
                    MergeOutcome::Conflict { files } => {
                        Ok(Err(format!("its merge conflicts in {}", files.join(", "))))
                    }
                }
            })();
            match step {
                Ok(Ok(commit)) => {
                    if let Some(r) = landing.repos.iter_mut().find(|r| r.repo == repo) {
                        r.state = LandState::Landed;
                        r.commit = commit;
                    }
                }
                Ok(Err(why)) => {
                    landing.fail(&repo, &why);
                    break;
                }
                Err(e) => {
                    landing.fail(&repo, &e.to_string());
                    break;
                }
            }
        }
        if landing.failed().is_some() {
            for (repo, workdir, before) in landed.iter().rev() {
                match merge.reset(workdir, before) {
                    Ok(()) => landing.undone(repo),
                    Err(e) => {
                        if let Some(r) = landing.repos.iter_mut().find(|r| &r.repo == repo) {
                            r.note = format!("could not be put back: {e}");
                        }
                    }
                }
            }
        }
        let summary = landing.summary();
        let failed = landing.failed().is_some();
        let m = &mut self.missions[i];
        m.landing = Some(landing);
        m.updated_at = self.clock.now();
        self.store.save(&self.missions[i])?;
        let event = if failed { "landing_failed" } else { "landed" };
        self.ledger_plan(&slug, event, &format!("{summary} into {into}"), None)?;
        Ok(failed.then_some(summary))
    }

    /// The set's worktrees in the order they land: as the mission names
    /// them, then any it did not name, in the workspace's order.
    fn landing_order(&self, i: usize, set: &[WorktreeRecord]) -> AppResult<Vec<WorktreeRecord>> {
        let m = &self.missions[i];
        let mut names: Vec<String> = m.contract.repos.clone();
        if let Some(w) = self.workspace_of(m)? {
            for repo in w.repo_names() {
                if !names.contains(&repo) {
                    names.push(repo);
                }
            }
        }
        let mut out: Vec<WorktreeRecord> = names
            .iter()
            .filter_map(|n| set.iter().find(|r| r.repo_name() == Some(n.as_str())))
            .cloned()
            .collect();
        for r in set {
            if !out.iter().any(|o| o.path == r.path) {
                out.push(r.clone());
            }
        }
        if out.is_empty() {
            return Err(AppError::Mission(format!(
                "{}: its job has no worktrees to land",
                m.slug
            )));
        }
        Ok(out)
    }

    /// Workspace missions that could not land: the user decides.
    pub(super) fn landing_items(&self) -> Vec<AttentionItem> {
        let mut out = Vec::new();
        for m in &self.missions {
            let Some(landing) = &m.landing else { continue };
            let Some(failed) = landing.failed() else {
                continue;
            };
            if m.state.in_flight() {
                continue;
            }
            let title = if m.contract.title.is_empty() {
                m.slug.as_str()
            } else {
                m.contract.title.as_str()
            };
            out.push(AttentionItem::merge_blocked(
                &m.slug,
                &failed.repo,
                (m.session, m.job),
                (title, &landing.summary()),
                landing.at.max(m.updated_at),
            ));
        }
        out
    }
}
