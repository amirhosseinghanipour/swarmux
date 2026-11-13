//! What the user can do about an outcome, as data.
//!
//! Only actions that exist today are listed. Every kind is always
//! answered, enabled or not, so a client can show a disabled action with
//! the reason instead of hiding it. There is no pause, no merge to main,
//! no per-file accept or revert: they do not exist.

use crate::JobStatus;
use crate::mission::MissionState;

/// An action the console can offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ActionKind {
    /// Stop a queued or running job.
    Kill,
    /// Queue a finished job's prompt again.
    Retry,
    /// Continue an interrupted or failed job where it stopped.
    Resume,
    /// Land the work in the mission's integration branch
    /// (`swarmux/<mission>/integration`, never the user's branches).
    LandToIntegration,
    /// Open the evidence bundle.
    OpenEvidence,
    /// Answer a held tool call: allow, deny or always allow.
    AnswerHeldCall,
    /// Approve the proposed plan and start the build.
    ApprovePlan,
    /// Send the proposed plan back with a note.
    RejectPlan,
}

impl ActionKind {
    /// Every kind, in the order they are answered.
    pub const ALL: [ActionKind; 8] = [
        ActionKind::Kill,
        ActionKind::Retry,
        ActionKind::Resume,
        ActionKind::LandToIntegration,
        ActionKind::OpenEvidence,
        ActionKind::AnswerHeldCall,
        ActionKind::ApprovePlan,
        ActionKind::RejectPlan,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ActionKind::Kill => "kill",
            ActionKind::Retry => "retry",
            ActionKind::Resume => "resume",
            ActionKind::LandToIntegration => "land_to_integration",
            ActionKind::OpenEvidence => "open_evidence",
            ActionKind::AnswerHeldCall => "answer_held_call",
            ActionKind::ApprovePlan => "approve_plan",
            ActionKind::RejectPlan => "reject_plan",
        }
    }
}

/// What the rules need to know about an outcome.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActionFacts {
    /// The mission's state; `None` for a plain job.
    pub mission: Option<MissionState>,
    /// The status of the outcome's lead job; `None` before it has one.
    pub job: Option<JobStatus>,
    /// Jobs of the outcome that are queued or running.
    pub live_jobs: usize,
    /// Tool calls held for the user.
    pub held_calls: usize,
    /// A plan waits for the user's decision.
    pub plan_proposed: bool,
    /// An evidence bundle was sealed.
    pub evidence: bool,
}

/// One action and whether it can be taken now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allowed {
    pub kind: ActionKind,
    pub enabled: bool,
    /// Why not, when it is disabled.
    pub reason: Option<String>,
}

fn yes(kind: ActionKind) -> Allowed {
    Allowed {
        kind,
        enabled: true,
        reason: None,
    }
}

fn no(kind: ActionKind, reason: &str) -> Allowed {
    Allowed {
        kind,
        enabled: false,
        reason: Some(reason.to_string()),
    }
}

fn when(kind: ActionKind, ok: bool, reason: &str) -> Allowed {
    if ok { yes(kind) } else { no(kind, reason) }
}

/// Every action kind for an outcome with these facts, enabled or not.
///
/// The preconditions are the ones the services check: a job can be
/// stopped while it is not finished, queued again once it is, and resumed
/// when it was interrupted or failed; a plan can be decided on while it
/// is proposed; a held call is answered while it is held.
///
/// Landing is the exception. Swarmux lands a mission's work itself, the
/// moment its gate passes, and there is no way to ask it to land on
/// request yet, so the action is never enabled; the reason says which of
/// the cases applies.
pub fn allowed(f: &ActionFacts) -> Vec<Allowed> {
    let delivered = f.mission == Some(MissionState::Delivered);
    ActionKind::ALL
        .into_iter()
        .map(|kind| match kind {
            ActionKind::Kill => when(
                kind,
                f.live_jobs > 0,
                if f.job.is_some() {
                    "its jobs have all ended"
                } else {
                    "no job has started"
                },
            ),
            ActionKind::Retry => match f.job {
                None => no(kind, "no job has run"),
                Some(s) if !s.is_finished() => no(kind, "the job has not ended"),
                Some(_) if delivered => no(kind, "the mission is delivered"),
                Some(_) => yes(kind),
            },
            ActionKind::Resume => when(
                kind,
                matches!(f.job, Some(JobStatus::Interrupted | JobStatus::Failed)),
                "only an interrupted or failed job can resume",
            ),
            ActionKind::LandToIntegration => match f.mission {
                None => no(kind, "a plain job has no integration branch"),
                Some(MissionState::Delivered) => no(kind, "the work is delivered"),
                Some(_) => no(
                    kind,
                    "swarmux lands the work itself when the gate passes; landing on request does not exist yet",
                ),
            },
            ActionKind::OpenEvidence => when(
                kind,
                f.evidence,
                "no evidence bundle yet: one is sealed when the gate passes",
            ),
            ActionKind::AnswerHeldCall => when(kind, f.held_calls > 0, "no call is held"),
            ActionKind::ApprovePlan | ActionKind::RejectPlan => {
                when(kind, f.plan_proposed, "no plan waits for a decision")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(list: &[Allowed], kind: ActionKind) -> &Allowed {
        list.iter().find(|a| a.kind == kind).unwrap()
    }

    #[test]
    fn every_kind_is_answered_once_in_order() {
        let list = allowed(&ActionFacts::default());
        let kinds: Vec<ActionKind> = list.iter().map(|a| a.kind).collect();
        assert_eq!(kinds, ActionKind::ALL);
        // A disabled action always says why; an enabled one never does.
        for a in &list {
            assert_eq!(a.enabled, a.reason.is_none(), "{:?}", a.kind);
        }
    }

    #[test]
    fn a_running_job_can_be_stopped_and_nothing_else() {
        let list = allowed(&ActionFacts {
            mission: Some(MissionState::Running),
            job: Some(JobStatus::Running),
            live_jobs: 1,
            ..ActionFacts::default()
        });
        let on: Vec<ActionKind> = list.iter().filter(|a| a.enabled).map(|a| a.kind).collect();
        assert_eq!(on, [ActionKind::Kill]);
    }

    #[test]
    fn a_failed_job_can_retry_and_resume_but_not_be_stopped() {
        let list = allowed(&ActionFacts {
            mission: None,
            job: Some(JobStatus::Failed),
            ..ActionFacts::default()
        });
        assert!(get(&list, ActionKind::Retry).enabled);
        assert!(get(&list, ActionKind::Resume).enabled);
        assert!(!get(&list, ActionKind::Kill).enabled);
    }

    #[test]
    fn a_done_or_cancelled_job_retries_but_cannot_resume() {
        for status in [JobStatus::Done, JobStatus::Cancelled] {
            let list = allowed(&ActionFacts {
                job: Some(status),
                ..ActionFacts::default()
            });
            assert!(get(&list, ActionKind::Retry).enabled, "{status:?}");
            assert!(!get(&list, ActionKind::Resume).enabled, "{status:?}");
        }
        let interrupted = allowed(&ActionFacts {
            job: Some(JobStatus::Interrupted),
            ..ActionFacts::default()
        });
        assert!(get(&interrupted, ActionKind::Resume).enabled);
    }

    #[test]
    fn a_delivered_mission_is_not_run_again() {
        let list = allowed(&ActionFacts {
            mission: Some(MissionState::Delivered),
            job: Some(JobStatus::Done),
            evidence: true,
            ..ActionFacts::default()
        });
        assert!(!get(&list, ActionKind::Retry).enabled);
        assert!(get(&list, ActionKind::OpenEvidence).enabled);
    }

    #[test]
    fn held_calls_and_plans_enable_their_answers() {
        let list = allowed(&ActionFacts {
            mission: Some(MissionState::Running),
            job: Some(JobStatus::Running),
            live_jobs: 1,
            held_calls: 2,
            plan_proposed: true,
            ..ActionFacts::default()
        });
        assert!(get(&list, ActionKind::AnswerHeldCall).enabled);
        assert!(get(&list, ActionKind::ApprovePlan).enabled);
        assert!(get(&list, ActionKind::RejectPlan).enabled);
        let none = allowed(&ActionFacts::default());
        assert!(!get(&none, ActionKind::AnswerHeldCall).enabled);
        assert!(!get(&none, ActionKind::ApprovePlan).enabled);
    }

    #[test]
    fn landing_on_request_is_never_enabled_and_says_why() {
        for mission in [
            None,
            Some(MissionState::Verifying),
            Some(MissionState::Escalated),
            Some(MissionState::Delivered),
        ] {
            let list = allowed(&ActionFacts {
                mission,
                job: Some(JobStatus::Done),
                ..ActionFacts::default()
            });
            let land = get(&list, ActionKind::LandToIntegration);
            assert!(!land.enabled, "{mission:?}");
            assert!(land.reason.as_deref().is_some_and(|r| !r.is_empty()));
        }
    }
}
