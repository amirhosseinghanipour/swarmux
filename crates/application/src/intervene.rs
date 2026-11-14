//! Intervene: the operator's message to a running agent.
//!
//! See [`swarmux_domain::intervene`] for the modes and what each promises.
//! The way a message reaches the agent is the way a send back does it: the
//! job's Claude session continues, in the same working folder, with the
//! message as its next prompt. swarmux never writes into a running
//! `claude`'s input.
//!
//! - **Queue** keeps the message in swarmux until the job's turn has ended,
//!   then continues the session ([`Engine::pump_interventions`] does it,
//!   once per frame).
//! - **Steer** stops the turn (the same stop as Kill) and continues the
//!   session at once. A call held for the operator is not stranded: steering
//!   is refused while one waits.
//!
//! The ledger is written first: a message that cannot be recorded is not
//! taken, and nothing is sent. A second entry says how it ended. A receipt
//! never says "delivered" before the agent's turn with the message has
//! started; between the hand-over and that start it says it does not know.
//!
//! Messages live in this run: a message still waiting when swarmux stops
//! is not delivered (the job is marked interrupted and the operator sends
//! it again, or sends the work back with a note).

use swarmux_domain::intervene::{
    Config, MAX_PENDING, Mode, Receipt, Refusal, State, clean, prompt,
};
use swarmux_domain::{JobId, JobStatus, Millis};

use crate::ledger::InterveneEntry;
use crate::{AppError, Engine};

/// Receipts kept in memory.
const KEEP: usize = 100;

/// The messages of this run and the settings they follow.
#[derive(Debug, Default)]
pub struct Interventions {
    config: Config,
    next: u64,
    items: Vec<Receipt>,
    /// Messages whose outcome was already reported by [`Interventions::news`].
    told: Vec<u64>,
}

impl Interventions {
    pub fn configure(&mut self, config: Config) {
        self.config = config;
    }

    pub fn config(&self) -> Config {
        self.config
    }

    /// The messages taken for `job`, oldest first.
    pub fn for_job(&self, job: JobId) -> impl Iterator<Item = &Receipt> {
        self.items.iter().filter(move |r| r.job == job)
    }

    /// The messages that concern `job`: those sent to it, and those handed
    /// to it as the job that carries them.
    pub fn involving(&self, job: JobId) -> impl Iterator<Item = &Receipt> {
        self.items.iter().filter(move |r| {
            r.job == job
                || matches!(
                    r.state,
                    State::Handed { job: j } | State::Delivered { job: j, .. } if j == job
                )
        })
    }

    /// Messages of `job` still waiting for the end of its turn.
    pub fn waiting(&self, job: JobId) -> usize {
        self.for_job(job)
            .filter(|r| r.state == State::Queued)
            .count()
    }

    /// What became of messages since this was last asked: one line for each
    /// that was delivered or could not be, said once. The screen shows it
    /// even when the job it was about is no longer on it.
    pub fn news(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        for r in &self.items {
            if self.told.contains(&r.id) {
                continue;
            }
            let line = match &r.state {
                State::Delivered { job, .. } => {
                    format!(
                        "message #{} delivered: the agent's turn with it started as job #{job}",
                        r.id
                    )
                }
                State::Undelivered(why) => format!("message #{} not delivered: {why}", r.id),
                _ => continue,
            };
            self.told.push(r.id);
            out.push(line);
        }
        self.told
            .retain(|id| self.items.iter().any(|r| r.id == *id));
        out
    }

    /// The receipt numbered `id`.
    pub fn get(&self, id: u64) -> Option<&Receipt> {
        self.items.iter().find(|r| r.id == id)
    }

    fn trim(&mut self) {
        while self.items.len() > KEEP {
            match self.items.iter().position(|r| r.state.is_settled()) {
                Some(i) => {
                    self.items.remove(i);
                }
                None => break,
            }
        }
    }
}

impl Engine {
    /// Takes the operator's `text` for the agent of `job`, in `mode`.
    ///
    /// `confirmed` is the operator's second confirmation, needed for
    /// [`Mode::Steer`] when the settings ask for one. Never fails: the
    /// receipt says whether the message was taken, refused (with the
    /// reason) or, later, delivered.
    pub fn intervene(&mut self, job: JobId, text: &str, mode: Mode, confirmed: bool) -> Receipt {
        let now = self.jobs.now();
        let config = self.interventions.config;
        let refuse = |text: &str, why: Refusal| Receipt {
            id: 0,
            job,
            mode,
            text: text.to_string(),
            state: State::Refused(why),
            at: now,
            seq: None,
        };
        let text = match clean(text, config.max_chars) {
            Ok(t) => t,
            Err(why) => return refuse(text, why),
        };
        if let Some(why) = self.cannot_intervene(job, mode, confirmed) {
            return refuse(&text, why);
        }
        let stored = match self.jobs.stored(job) {
            Ok(Some(j)) => j,
            _ => return refuse(&text, Refusal::NoSuchJob),
        };
        let id = self.interventions.next + 1;
        let mission = self.mission_of(job);
        // Recorded before anything happens: no record, no message.
        let seq = match &self.ledger {
            Some(ledger) => {
                let outcome = match mode {
                    Mode::Queue => "queued",
                    Mode::Steer => "stopping",
                };
                let entry = InterveneEntry {
                    mode: mode.as_str(),
                    note: Some(&text),
                    job,
                    session: Some(stored.session_id),
                    outcome,
                    message: id,
                    queued_seq: None,
                    mission: mission.as_deref(),
                    resumed_as: None,
                    delivered_at: None,
                    reason: None,
                };
                match ledger.record_intervention(&entry, now) {
                    Ok(seq) => Some(seq),
                    Err(e) => return refuse(&text, Refusal::Ledger(e.to_string())),
                }
            }
            None => None,
        };
        self.interventions.next = id;
        self.interventions.items.push(Receipt {
            id,
            job,
            mode,
            text: text.clone(),
            state: State::Queued,
            at: now,
            seq,
        });
        self.interventions.trim();
        self.jobs
            .operator_line(job, "message", &format!("[{}] {text}", mode.as_str()));
        if mode == Mode::Steer
            && let Err(e) = self.jobs.cancel(job)
        {
            let why = Refusal::CouldNotStop(e.to_string());
            let mut errors = Vec::new();
            self.fail_all(&[id], &why.reason(), &mut errors);
            return self
                .interventions
                .get(id)
                .cloned()
                .unwrap_or_else(|| refuse(&text, why));
        }
        // A steered turn has ended; deliver now rather than at the next frame.
        let _ = self.pump_interventions();
        self.interventions
            .get(id)
            .cloned()
            .unwrap_or_else(|| refuse(&text, Refusal::NoSuchJob))
    }

    /// Why `job` cannot be messaged in `mode` now.
    fn cannot_intervene(&self, job: JobId, mode: Mode, confirmed: bool) -> Option<Refusal> {
        if self.jobs.draining() {
            return Some(Refusal::Stopping);
        }
        let stored = match self.jobs.stored(job) {
            Ok(Some(j)) => j,
            _ => return Some(Refusal::NoSuchJob),
        };
        if stored.review_of.is_some() {
            return Some(Refusal::Reviewer);
        }
        let Some(progress) = self.jobs.progress(job) else {
            // Stored as running, but not by this run: no process to follow.
            return Some(if stored.status == JobStatus::Running {
                Refusal::NotControlled
            } else {
                Refusal::NotRunning(stored.status)
            });
        };
        if progress.status != JobStatus::Running {
            return Some(Refusal::NotRunning(progress.status));
        }
        if self.in_task_graph(job) {
            return Some(Refusal::TaskGraph);
        }
        let waiting = self.interventions.waiting(job);
        if waiting >= MAX_PENDING {
            return Some(Refusal::TooMany(waiting));
        }
        if mode == Mode::Steer {
            let held = self
                .guard
                .approvals()
                .iter()
                .any(|a| a.request.job_id == Some(job));
            if held {
                return Some(Refusal::ApprovalPending);
            }
            if self.interventions.config.confirm_steer && !confirmed {
                return Some(Refusal::ConfirmSteer);
            }
        }
        None
    }

    fn in_task_graph(&self, job: JobId) -> bool {
        let Some(missions) = &self.missions else {
            return false;
        };
        missions.missions().iter().any(|m| {
            missions
                .graph(&m.slug)
                .is_some_and(|g| g.task_of(job).is_some())
        })
    }

    fn mission_of(&self, job: JobId) -> Option<String> {
        self.missions
            .as_ref()?
            .missions()
            .iter()
            .find(|m| m.job == Some(job))
            .map(|m| m.slug.clone())
    }

    /// Moves the messages along: hands over those whose job's turn has
    /// ended, and confirms those whose continuation has started. Called
    /// every frame after the missions were pumped (a mission's job must
    /// have been taken in before the message can follow it). Returns what
    /// went wrong while recording.
    pub fn pump_interventions(&mut self) -> Vec<AppError> {
        let mut errors = Vec::new();
        if self
            .interventions
            .items
            .iter()
            .all(|r| r.state.is_settled())
        {
            return errors;
        }
        let now = self.jobs.now();
        self.confirm_handed(now, &mut errors);
        let mut due: Vec<JobId> = Vec::new();
        for r in &self.interventions.items {
            if r.state == State::Queued && !due.contains(&r.job) {
                due.push(r.job);
            }
        }
        for job in due {
            self.hand_over(job, &mut errors);
        }
        errors
    }

    /// A message handed to a continuation: delivered once that job's agent
    /// has reported, not delivered if it ended without.
    fn confirm_handed(&mut self, now: Millis, errors: &mut Vec<AppError>) {
        let handed: Vec<(u64, JobId)> = self
            .interventions
            .items
            .iter()
            .filter_map(|r| match r.state {
                State::Handed { job } => Some((r.id, job)),
                _ => None,
            })
            .collect();
        for (id, new) in handed {
            let state = if self.jobs.has_reported(new) {
                State::Delivered { job: new, at: now }
            } else {
                match self.jobs.progress(new).map(|p| p.status) {
                    Some(s) if s.is_finished() => State::Undelivered(format!(
                        "job #{new}, which carries it, ended ({}) before the agent reported",
                        s.as_str()
                    )),
                    None => State::Undelivered(format!("job #{new} is no longer followed")),
                    _ => continue,
                }
            };
            if let Err(e) = self.settle_recorded(id, state) {
                errors.push(e);
            }
        }
    }

    /// Hands the waiting messages of `job` to its agent, when its turn has
    /// ended and the way is open; marks them not delivered when it will
    /// not open.
    fn hand_over(&mut self, job: JobId, errors: &mut Vec<AppError>) {
        let waiting: Vec<u64> = self
            .interventions
            .for_job(job)
            .filter(|r| r.state == State::Queued)
            .map(|r| r.id)
            .collect();
        let steered = self
            .interventions
            .for_job(job)
            .any(|r| r.state == State::Queued && r.mode == Mode::Steer);
        let Some(status) = self.jobs.progress(job).map(|p| p.status) else {
            return self.fail_all(&waiting, "swarmux no longer follows the job", errors);
        };
        if !status.is_finished() {
            return;
        }
        if status == JobStatus::Cancelled && !steered {
            return self.fail_all(
                &waiting,
                "the job was stopped before its turn ended: send it back with a note",
                errors,
            );
        }
        if let Some(missions) = &self.missions
            && let Some(m) = missions.missions().iter().find(|m| m.job == Some(job))
        {
            use swarmux_domain::mission::MissionState as S;
            match m.state {
                // The mission has not taken the ended job in yet.
                S::Running | S::Queued => return,
                S::Escalated | S::Verifying => {}
                S::Delivered => {
                    return self.fail_all(
                        &waiting,
                        "the mission was delivered before the message could be: change it with a new mission",
                        errors,
                    );
                }
                other => {
                    return self.fail_all(
                        &waiting,
                        &format!("the mission is {other}: the message cannot follow it"),
                        errors,
                    );
                }
            }
        }
        let mode = if steered { Mode::Steer } else { Mode::Queue };
        let texts: Vec<String> = waiting
            .iter()
            .filter_map(|id| self.interventions.get(*id).map(|r| r.text.clone()))
            .collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        match self.resume_with_prompt(job, &prompt(mode, &refs)) {
            Ok((new, _, _)) => {
                for id in waiting {
                    self.set_state(id, State::Handed { job: new });
                }
                self.jobs
                    .note(new, "message", "this turn carries your message");
            }
            Err(e) => self.fail_all(&waiting, &e.to_string(), errors),
        }
    }

    fn fail_all(&mut self, ids: &[u64], why: &str, errors: &mut Vec<AppError>) {
        for id in ids {
            if let Err(e) = self.settle_recorded(*id, State::Undelivered(why.to_string())) {
                errors.push(e);
            }
        }
    }

    fn set_state(&mut self, id: u64, state: State) {
        if let Some(r) = self.interventions.items.iter_mut().find(|r| r.id == id) {
            r.state = state;
        }
    }

    /// Changes a message's state and records how it ended. The message was
    /// recorded when it was taken; this entry says what became of it, and
    /// when the agent's turn with it started.
    fn settle_recorded(&mut self, id: u64, state: State) -> Result<(), AppError> {
        self.set_state(id, state.clone());
        let (Some(ledger), Some(r)) = (&self.ledger, self.interventions.get(id).cloned()) else {
            return Ok(());
        };
        let (outcome, resumed, at, reason) = match &state {
            State::Delivered { job, at } => ("delivered", Some(*job), Some(*at), None),
            State::Undelivered(why) => ("undelivered", None, None, Some(why.as_str())),
            _ => return Ok(()),
        };
        let mission = self.mission_of(r.job);
        let session = self.jobs.stored(r.job).ok().flatten().map(|j| j.session_id);
        let entry = InterveneEntry {
            mode: r.mode.as_str(),
            note: None,
            job: r.job,
            session,
            outcome,
            message: r.id,
            queued_seq: r.seq,
            mission: mission.as_deref(),
            resumed_as: resumed,
            delivered_at: at,
            reason,
        };
        ledger
            .record_intervention(&entry, self.jobs.now())
            .map(drop)
            .map_err(|e| {
                AppError::Storage(format!(
                    "the message's outcome ({outcome}) could not be recorded in the ledger: {e}"
                ))
            })
    }
}

#[cfg(test)]
mod tests;
