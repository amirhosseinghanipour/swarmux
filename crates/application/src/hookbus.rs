//! The hook bus: lifecycle events Claude Code reports through its hooks,
//! recorded in the ledger and condensed into per-job signals.
//!
//! `PreToolUse` keeps its own path (the Guard decides it); everything else
//! arrives here as a [`HookNotice`].

use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use swarmux_domain::telemetry::InstructionFile;
use swarmux_domain::{CheckpointReason, HookEvent, JobId, Millis, SessionId};

use crate::attention::{Change, ChangeKind};
use crate::checkpoint::{CheckpointService, Notes};
use crate::ledger::LedgerService;
use crate::ports::{Clock, JobRepository};
use crate::telemetry::TruthBoard;
use crate::{AppError, AppResult};

/// Changes the bus keeps for the change log.
const CHANGES_KEPT: usize = 100;

/// One lifecycle event, as the transport condensed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookNotice {
    pub event: HookEvent,
    /// The swarmux job whose `claude` reported it, when known.
    pub job: Option<JobId>,
    /// Claude Code's own session id.
    pub session: String,
    /// The session's working folder.
    pub cwd: String,
    pub tool: Option<String>,
    /// One line saying what happened.
    pub summary: String,
    /// The part signals are built from: the denial reason, the failure,
    /// the compaction trigger or the notification type.
    pub detail: String,
    /// SHA-256 of the whole hook input, and its length in bytes.
    pub input_sha256: String,
    pub input_len: usize,
    /// The hook input as received, cut at 16 KB.
    pub raw: String,
}

/// Where lifecycle events arrive.
pub trait HookFeed {
    /// Events received since the last call.
    fn poll_notices(&self) -> Vec<HookNotice>;
}

/// What a `Stop` or `SubagentStop` hook is told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StopContext {
    pub event: HookEvent,
    pub job: Option<JobId>,
    pub session: String,
    /// The session's working folder.
    pub cwd: String,
    pub last_message: String,
}

/// The answer to a `Stop` or `SubagentStop` hook.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopAnswer {
    /// Let the agent stop.
    Allow,
    /// Make it continue, with `reason` as the instruction.
    Block { reason: String },
}

/// Decides whether an agent may stop. The default allows everything.
///
/// Called on the hook transport's threads, where it may take its time.
pub trait StopPolicy: Send + Sync {
    fn on_stop(&self, stop: &StopContext) -> StopAnswer;

    /// Sees every other lifecycle event first (a session starting, …).
    fn observe(&self, _notice: &HookNotice) {}
}

/// What a `SessionStart` hook is told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionStartInfo {
    pub job: Option<JobId>,
    pub session: String,
    /// `startup`, `resume`, `clear` or `compact`.
    pub source: String,
}

/// Text added to a session's context when it starts. The default adds none.
pub trait SessionStartContext: Send + Sync {
    fn context(&self, start: &SessionStartInfo) -> Option<String>;
}

/// The default [`StopPolicy`]: always allows.
#[derive(Clone, Copy, Debug, Default)]
pub struct AllowStop;

impl StopPolicy for AllowStop {
    fn on_stop(&self, _: &StopContext) -> StopAnswer {
        StopAnswer::Allow
    }
}

/// The default [`SessionStartContext`]: adds nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoSessionContext;

impl SessionStartContext for NoSessionContext {
    fn context(&self, _: &SessionStartInfo) -> Option<String> {
        None
    }
}

/// What the bus knows about one job's session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionSignals {
    pub last_event_at: Option<Millis>,
    pub compactions: u32,
    pub last_tool: Option<String>,
    pub permission_denied_count: u32,
    pub last_stop_failure: Option<String>,
    /// Notifications that said the agent needs the user.
    pub needs_input: u32,
}

/// Records lifecycle events and keeps their signals.
pub struct HookBus {
    feed: Rc<dyn HookFeed>,
    ledger: Rc<LedgerService>,
    jobs: Rc<dyn JobRepository>,
    clock: Rc<dyn Clock>,
    signals: HashMap<JobId, SessionSignals>,
    changes: VecDeque<Change>,
    checkpoints: Option<Rc<CheckpointService>>,
    truth: Option<Rc<TruthBoard>>,
}

impl HookBus {
    pub fn new(
        feed: Rc<dyn HookFeed>,
        ledger: Rc<LedgerService>,
        jobs: Rc<dyn JobRepository>,
        clock: Rc<dyn Clock>,
    ) -> HookBus {
        HookBus {
            feed,
            ledger,
            jobs,
            clock,
            signals: HashMap::new(),
            changes: VecDeque::new(),
            checkpoints: None,
            truth: None,
        }
    }

    /// Keeps the instruction files sessions load on `board`.
    pub fn with_truth(mut self, board: Rc<TruthBoard>) -> HookBus {
        self.truth = Some(board);
        self
    }

    /// Takes a checkpoint of the job's worktree before each compaction.
    pub fn with_checkpoints(mut self, checkpoints: Rc<CheckpointService>) -> HookBus {
        self.checkpoints = Some(checkpoints);
        self
    }

    /// Takes the events that arrived and dispatches them. Returns what
    /// could not be recorded.
    pub fn pump(&mut self) -> Vec<AppError> {
        let mut errors = Vec::new();
        for notice in self.feed.poll_notices() {
            errors.extend(self.dispatch(&notice).err());
        }
        errors
    }

    /// Records one event in the ledger and updates the job's signals.
    /// `PreToolUse` belongs to the Guard and is ignored here.
    pub fn dispatch(&mut self, notice: &HookNotice) -> AppResult<()> {
        if notice.event == HookEvent::PreToolUse {
            return Ok(());
        }
        let now = self.clock.now();
        self.observe(notice, now);
        let session = match notice.job {
            Some(job) => self.jobs.get(job)?.map(|j| j.session_id),
            None => None,
        };
        self.record(notice, session, now)?;
        self.checkpoint_before_compaction(notice)
    }

    fn checkpoint_before_compaction(&self, n: &HookNotice) -> AppResult<()> {
        let (Some(checkpoints), Some(job), HookEvent::PreCompact) =
            (&self.checkpoints, n.job, n.event)
        else {
            return Ok(());
        };
        let trigger = if n.detail.is_empty() {
            "auto"
        } else {
            &n.detail
        };
        let notes = Notes {
            state: format!("The context was about to be compacted ({trigger})."),
            next_step: "Continue the task; re-read the files you need after the compaction.".into(),
            ..Notes::default()
        };
        checkpoints
            .take(job, &n.session, CheckpointReason::PreCompact, &notes)
            .map(drop)
    }

    fn record(&self, n: &HookNotice, session: Option<SessionId>, now: Millis) -> AppResult<()> {
        self.ledger.record_hook_event(n, session, now)
    }

    fn observe(&mut self, n: &HookNotice, now: Millis) {
        if let (HookEvent::InstructionsLoaded, Some(board)) = (n.event, &self.truth) {
            let file = InstructionFile::from_notice(&n.summary, &n.detail, now);
            board.record_instruction(&n.session, n.job, file);
        }
        if let Some(job) = n.job {
            let s = self.signals.entry(job).or_default();
            s.last_event_at = Some(now);
            if let Some(tool) = &n.tool {
                s.last_tool = Some(tool.clone());
            }
            match n.event {
                HookEvent::PreCompact => s.compactions += 1,
                HookEvent::PermissionDenied => s.permission_denied_count += 1,
                HookEvent::StopFailure => s.last_stop_failure = Some(n.detail.clone()),
                HookEvent::Notification if needs_user(&n.detail) => s.needs_input += 1,
                _ => {}
            }
        }
        let change = match n.event {
            HookEvent::PermissionDenied => Some((
                ChangeKind::Denied,
                format!("classifier denied {}", n.summary),
            )),
            HookEvent::StopFailure => Some((
                ChangeKind::Failed,
                format!(
                    "turn failed: {}",
                    if n.detail.is_empty() {
                        "error"
                    } else {
                        &n.detail
                    }
                ),
            )),
            _ => None,
        };
        if let Some((kind, text)) = change {
            if self.changes.len() == CHANGES_KEPT {
                self.changes.pop_front();
            }
            self.changes.push_back(Change {
                at: now,
                job: n.job,
                session: String::new(),
                kind,
                text,
            });
        }
    }

    /// What is known about `job`'s session.
    pub fn signals(&self, job: JobId) -> Option<&SessionSignals> {
        self.signals.get(&job)
    }

    /// Permission denials and failed turns, oldest first. Session names are
    /// empty; the attention service fills them in.
    pub fn changes(&self) -> Vec<Change> {
        self.changes.iter().cloned().collect()
    }
}

/// Notification types that mean the agent waits for the user.
fn needs_user(kind: &str) -> bool {
    matches!(
        kind,
        "permission_prompt" | "idle_prompt" | "agent_needs_input" | "elicitation_dialog"
    )
}

#[cfg(test)]
mod tests;
