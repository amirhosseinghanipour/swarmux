//! The step a job is at, from plan to ship, derived from what it did.

use crate::{JobStatus, ReviewState, ReviewVerdict};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Phase {
    /// The lead works alone: no subagent and no guard decision yet.
    Plan,
    /// The first guard, routing or JEV decisions are being made.
    Fork,
    /// One subagent is running.
    Delegate,
    /// Two or more subagents run at once.
    Parallel,
    /// Subagents finished; the lead carries on.
    Merge,
    /// The reviewer is checking the job (or raised concerns).
    Audit,
    /// Done, and the review passed if there was one.
    Ship,
}

impl Phase {
    pub const ALL: [Phase; 7] = [
        Phase::Plan,
        Phase::Fork,
        Phase::Delegate,
        Phase::Parallel,
        Phase::Merge,
        Phase::Audit,
        Phase::Ship,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Plan => "plan",
            Phase::Fork => "fork",
            Phase::Delegate => "delegate",
            Phase::Parallel => "parallel",
            Phase::Merge => "merge",
            Phase::Audit => "audit",
            Phase::Ship => "ship",
        }
    }

    /// Position in [`Phase::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }
}

/// What a job's phase depends on.
#[derive(Clone, Copy, Debug)]
pub struct PhaseInput<'a> {
    pub status: JobStatus,
    /// Guard, routing and JEV decisions made for the job.
    pub decisions: u32,
    pub subagents_started: usize,
    pub subagents_running: usize,
    pub review: Option<&'a ReviewState>,
}

/// The job's current phase.
pub fn phase_of(input: PhaseInput) -> Phase {
    if input.status == JobStatus::Done {
        return match input.review {
            Some(ReviewState::Requested | ReviewState::Running { .. }) => Phase::Audit,
            Some(ReviewState::Done { review, .. }) if review.verdict == ReviewVerdict::Concerns => {
                Phase::Audit
            }
            _ => Phase::Ship,
        };
    }
    match (input.subagents_running, input.subagents_started) {
        (2.., _) => Phase::Parallel,
        (1, _) => Phase::Delegate,
        (0, 1..) => Phase::Merge,
        _ if input.decisions > 0 => Phase::Fork,
        _ => Phase::Plan,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{JobId, Review};

    fn input(status: JobStatus) -> PhaseInput<'static> {
        PhaseInput {
            status,
            decisions: 0,
            subagents_started: 0,
            subagents_running: 0,
            review: None,
        }
    }

    fn done(verdict: ReviewVerdict) -> ReviewState {
        ReviewState::Done {
            review: Review {
                verdict,
                concerns: vec![],
                diffs_checked: 1,
            },
            job: JobId(2),
            cost_usd: 0.01,
        }
    }

    #[test]
    fn running_phases_follow_the_work() {
        let r = JobStatus::Running;
        assert_eq!(phase_of(input(r)), Phase::Plan);
        assert_eq!(
            phase_of(PhaseInput {
                decisions: 2,
                ..input(r)
            }),
            Phase::Fork
        );
        let one = PhaseInput {
            decisions: 2,
            subagents_started: 1,
            subagents_running: 1,
            ..input(r)
        };
        assert_eq!(phase_of(one), Phase::Delegate);
        let two = PhaseInput {
            subagents_started: 3,
            subagents_running: 2,
            ..one
        };
        assert_eq!(phase_of(two), Phase::Parallel);
        let merged = PhaseInput {
            subagents_running: 0,
            ..two
        };
        assert_eq!(phase_of(merged), Phase::Merge);
    }

    #[test]
    fn done_jobs_audit_then_ship() {
        let d = JobStatus::Done;
        assert_eq!(phase_of(input(d)), Phase::Ship);
        let running = ReviewState::Running { job: JobId(2) };
        assert_eq!(
            phase_of(PhaseInput {
                review: Some(&running),
                ..input(d)
            }),
            Phase::Audit
        );
        let pass = done(ReviewVerdict::Pass);
        assert_eq!(
            phase_of(PhaseInput {
                review: Some(&pass),
                ..input(d)
            }),
            Phase::Ship
        );
        let concerns = done(ReviewVerdict::Concerns);
        assert_eq!(
            phase_of(PhaseInput {
                review: Some(&concerns),
                ..input(d)
            }),
            Phase::Audit
        );
        let failed = ReviewState::Failed {
            error: "x".into(),
            job: None,
            cost_usd: 0.0,
        };
        assert_eq!(
            phase_of(PhaseInput {
                review: Some(&failed),
                ..input(d)
            }),
            Phase::Ship
        );
    }

    #[test]
    fn failed_jobs_keep_the_step_they_reached() {
        let f = PhaseInput {
            subagents_started: 1,
            ..input(JobStatus::Failed)
        };
        assert_eq!(phase_of(f), Phase::Merge);
    }

    #[test]
    fn names_and_order() {
        let names: Vec<&str> = Phase::ALL.iter().map(|p| p.as_str()).collect();
        assert_eq!(
            names,
            [
                "plan", "fork", "delegate", "parallel", "merge", "audit", "ship"
            ]
        );
        assert_eq!(Phase::Audit.index(), 5);
    }
}
