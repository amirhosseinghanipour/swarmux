//! Send back: the operator's note to an agent.
//!
//! Two things can be sent back with a note, and the note is the operator's
//! own words in both; swarmux adds nothing to it and never sends one by
//! itself (the screen asks the operator to confirm first).
//!
//! - **Work.** A job that stopped (failed, was interrupted or cancelled),
//!   was escalated, or finished and waits for the operator (verified, not
//!   accepted) carries on in the same Claude session and the same
//!   worktree, with the note as its next instruction ([`JobService::send_back`],
//!   which is the resume path with another prompt). If the job belongs to
//!   a mission the mission follows the new job (`Signal::SentBack`), and
//!   the gate judges the agent's next claim with the checks run again.
//! - **A held call.** The call is denied, and the note is delivered with
//!   the denial: Claude Code shows the denial's reason to the agent.
//!
//! Either way a ledger entry of kind `operator` records who (the actor
//! `operator`), when (its `ts`), the note, the job, what state it was in
//! and what happened next. It is chained like every entry.
//!
//! [`JobService::send_back`]: crate::JobService::send_back

use swarmux_domain::JobId;

use crate::ledger::OperatorNote;
use crate::{AppError, AppResult, Engine};

/// What [`Engine::send_back`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentBack {
    /// The job that was sent back.
    pub from: JobId,
    /// The job the agent carries on as.
    pub job: JobId,
    /// The mission that follows the new job, when the work was one's.
    pub mission: Option<String>,
    /// The ledger entry that records the note, when there is a ledger.
    pub seq: Option<u64>,
}

/// The operator's note as the agent receives it: their words, framed so the
/// agent knows where they came from and what happens next.
pub fn send_back_prompt(note: &str) -> String {
    format!(
        "The operator read your work and sent it back with this note:\n\n{}\n\nCarry on in the same session and the same working folder. When you are done, say so as before: the checks run again on your next claim.",
        note.trim()
    )
}

fn need_words(note: &str) -> AppResult<&str> {
    let note = note.trim();
    if note.is_empty() {
        return Err(AppError::Mission(
            "a note needs words: say what to do differently".into(),
        ));
    }
    Ok(note)
}

impl Engine {
    /// Sends `job`'s work back to its agent with `note` (see the module
    /// docs). The job must have ended; a mission's must be escalated or
    /// verifying.
    pub fn send_back(&mut self, job: JobId, note: &str) -> AppResult<SentBack> {
        let note = need_words(note)?;
        let stored = self.jobs.stored(job)?.ok_or(AppError::JobNotFound)?;
        let (new, mission, outcome) = self.resume_with_prompt(job, &send_back_prompt(note))?;
        let seq = self.record_note(&OperatorNote {
            action: "send_back",
            note,
            job: Some(job),
            session: Some(stored.session_id),
            outcome,
            mission: mission.as_deref(),
            resumed_as: Some(new),
            request: None,
            call: None,
            gate_reruns: true,
        })?;
        Ok(SentBack {
            from: job,
            job: new,
            mission,
            seq,
        })
    }

    /// Continues `job`'s Claude session with `prompt` as its next instruction,
    /// the way a send back does: through its mission when it belongs to one
    /// (which must be escalated or verifying), else straight through the
    /// job service. Returns the new job, the mission that follows it and
    /// where the job or mission stood (`escalated`, `verifying`, `done` ...).
    pub(crate) fn resume_with_prompt(
        &mut self,
        job: JobId,
        prompt: &str,
    ) -> AppResult<(JobId, Option<String>, &'static str)> {
        let stored = self.jobs.stored(job)?.ok_or(AppError::JobNotFound)?;
        let slug = self
            .missions
            .as_ref()
            .and_then(|m| m.missions().iter().find(|m| m.job == Some(job)))
            .map(|m| (m.slug.clone(), m.state));
        Ok(match (slug, &mut self.missions) {
            (Some((slug, state)), Some(missions)) => {
                let new = missions.send_back(&slug, &mut self.jobs, prompt)?;
                (new, Some(slug), state.as_str())
            }
            _ => {
                let new = self.jobs.send_back(job, prompt)?;
                (new, None, stored.status.as_str())
            }
        })
    }

    /// Denies the held call `request_id` and delivers `note` with the
    /// denial. Returns the ledger entry that records the note, when there
    /// is a ledger.
    pub fn send_back_held(&mut self, request_id: u64, note: &str) -> AppResult<Option<u64>> {
        let note = need_words(note)?;
        let held = self
            .guard
            .approvals()
            .into_iter()
            .find(|a| a.request.request_id == request_id)
            .ok_or(AppError::ApprovalNotFound)?;
        self.guard.deny_with_note(request_id, note)?;
        let job = held.request.job_id;
        let session = match job {
            Some(j) => self.jobs.stored(j)?.map(|j| j.session_id),
            None => None,
        };
        let call = format!("{} {}", held.request.tool_name, held.request.subject);
        self.record_note(&OperatorNote {
            action: "deny_with_note",
            note,
            job,
            session,
            outcome: "held",
            mission: None,
            resumed_as: None,
            request: Some(request_id),
            call: Some(call.trim()),
            gate_reruns: false,
        })
    }

    /// Writes the note to the ledger. The agent has been told already: when
    /// the entry cannot be written the error says so, and does not pretend
    /// the note was not sent.
    fn record_note(&self, note: &OperatorNote<'_>) -> AppResult<Option<u64>> {
        let Some(ledger) = &self.ledger else {
            return Ok(None);
        };
        // The clock the work runs by is the ledger service's own.
        ledger
            .record_operator_note(note, ledger.now())
            .map(Some)
            .map_err(|e| {
                AppError::Storage(format!(
                    "the note was delivered, but not recorded in the ledger: {e}"
                ))
            })
    }
}

#[cfg(test)]
mod tests;
