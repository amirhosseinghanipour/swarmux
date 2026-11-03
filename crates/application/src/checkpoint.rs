//! Checkpoints: resumable points of a job, taken when the done-gate passes
//! and before Claude Code compacts its context.
//!
//! A checkpoint is the Claude session id, a commit holding the worktree's
//! files, the ledger entry that records it, and a handoff for whoever picks
//! the work up.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use swarmux_domain::{Checkpoint, CheckpointReason, Handoff, JobId};

use crate::AppResult;
use crate::hookbus::{SessionStartContext, SessionStartInfo};
use crate::ledger::LedgerService;
use crate::ports::{Clock, JobRepository};
use crate::worktree::WorktreeService;

/// Longest goal kept in a handoff, in characters.
const GOAL_MAX: usize = 600;
/// Steps of the job's log that go into a handoff.
const STEPS_KEPT: usize = 6;
/// Longest step line, in characters.
const STEP_MAX: usize = 160;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Handoffs waiting for the session of a job to start, shared with the hook
/// transport's threads.
#[derive(Default)]
pub struct HandoffBoard {
    briefings: Mutex<HashMap<JobId, String>>,
}

impl HandoffBoard {
    pub fn new() -> Arc<HandoffBoard> {
        Arc::new(HandoffBoard::default())
    }

    pub fn set(&self, job: JobId, briefing: &str) {
        lock(&self.briefings).insert(job, briefing.to_string());
    }

    pub fn get(&self, job: JobId) -> Option<String> {
        lock(&self.briefings).get(&job).cloned()
    }
}

impl SessionStartContext for HandoffBoard {
    fn context(&self, start: &SessionStartInfo) -> Option<String> {
        // A brand new session has nothing to be handed.
        if start.source == "startup" {
            return None;
        }
        self.get(start.job?)
    }
}

/// What the caller knows about why the checkpoint is taken.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notes {
    /// How the job stands, one line.
    pub state: String,
    /// Things that were tried and did not work.
    pub tried_and_failed: Vec<String>,
    /// What to do next, when known.
    pub next_step: String,
}

/// Takes checkpoints and lists them.
pub struct CheckpointService {
    worktrees: Rc<WorktreeService>,
    jobs: Rc<dyn JobRepository>,
    ledger: Rc<LedgerService>,
    clock: Rc<dyn Clock>,
    board: Option<Arc<HandoffBoard>>,
}

impl CheckpointService {
    pub fn new(
        worktrees: Rc<WorktreeService>,
        jobs: Rc<dyn JobRepository>,
        ledger: Rc<LedgerService>,
        clock: Rc<dyn Clock>,
    ) -> CheckpointService {
        CheckpointService {
            worktrees,
            jobs,
            ledger,
            clock,
            board: None,
        }
    }

    /// Hands every new handoff to the sessions that start next.
    pub fn with_handoffs(mut self, board: Arc<HandoffBoard>) -> CheckpointService {
        self.board = Some(board);
        self
    }

    /// Commits the job's worktree, writes the handoff and records the
    /// checkpoint in the ledger. `None` when the job has no worktree.
    pub fn take(
        &self,
        job: JobId,
        claude_session: &str,
        reason: CheckpointReason,
        notes: &Notes,
    ) -> AppResult<Option<Checkpoint>> {
        let Some(record) = self.worktrees.record_for(job)? else {
            return Ok(None);
        };
        let git = self.worktrees.git();
        let commit = git.snapshot(
            &record.path,
            &format!("swarmux checkpoint ({}) for job {job}", reason.as_str()),
        )?;
        let handoff = self.handoff(job, notes)?;
        // The handoff is a convenience: a checkpoint without the file still
        // resumes, so a write problem must not lose the checkpoint.
        let file = git.write_handoff(&record.path, &handoff).ok();
        let session_id = if claude_session.is_empty() {
            record.session.to_string()
        } else {
            claude_session.to_string()
        };
        let now = self.clock.now();
        let mut checkpoint = Checkpoint {
            id: 0,
            job,
            session_id,
            commit,
            worktree: record.path.clone(),
            branch: record.branch.clone(),
            reason,
            ledger_seq: None,
            handoff: Some(handoff),
            at: now,
        };
        let file = file.map(|p| p.display().to_string());
        let seq =
            self.ledger
                .record_checkpoint(&checkpoint, file.as_deref(), Some(record.session))?;
        checkpoint.ledger_seq = Some(seq);
        checkpoint.id = self.worktrees.store().insert_checkpoint(&checkpoint)?;
        if let (Some(board), Some(handoff)) = (&self.board, &checkpoint.handoff) {
            board.set(job, &handoff.briefing());
        }
        Ok(Some(checkpoint))
    }

    fn handoff(&self, job: JobId, notes: &Notes) -> AppResult<Handoff> {
        let goal = self
            .jobs
            .get(job)?
            .map(|j| j.prompt)
            .unwrap_or_default()
            .chars()
            .take(GOAL_MAX)
            .collect();
        let mut steps: Vec<String> = self
            .jobs
            .events(job)?
            .into_iter()
            .filter(|e| e.kind == "tool_use")
            .map(|e| e.payload.chars().take(STEP_MAX).collect())
            .collect();
        let skip = steps.len().saturating_sub(STEPS_KEPT);
        steps.drain(..skip);
        let mut state = notes.state.clone();
        if !steps.is_empty() {
            state.push_str(&format!(" Last steps: {}.", steps.join("; ")));
        }
        Ok(Handoff {
            goal,
            state: state.trim().to_string(),
            decisions: Vec::new(),
            tried_and_failed: notes.tried_and_failed.clone(),
            next_step: if notes.next_step.is_empty() {
                "Carry on from the committed state of the worktree.".into()
            } else {
                notes.next_step.clone()
            },
            open_questions: Vec::new(),
        })
    }

    /// The checkpoints of `job`, oldest first.
    pub fn list(&self, job: JobId) -> AppResult<Vec<Checkpoint>> {
        self.worktrees.store().checkpoints(job)
    }

    /// The newest checkpoint of `job`.
    pub fn latest(&self, job: JobId) -> AppResult<Option<Checkpoint>> {
        Ok(self.list(job)?.pop())
    }

    /// The newest `limit` checkpoints of any job, newest first.
    pub fn recent(&self, limit: usize) -> AppResult<Vec<Checkpoint>> {
        self.worktrees.store().recent_checkpoints(limit)
    }
}

#[cfg(test)]
mod tests;
