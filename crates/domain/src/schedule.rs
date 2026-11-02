//! Which queued jobs may start now.

use crate::{Job, JobId, SessionId};

/// How much may run at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_concurrent: usize,
    /// Nothing starts while free memory is below this many bytes.
    pub min_free_ram: u64,
}

/// Queued jobs in the order they will run: highest priority first, then oldest.
pub fn run_order<'a>(queued: impl IntoIterator<Item = &'a Job>) -> Vec<&'a Job> {
    let mut jobs: Vec<&Job> = queued.into_iter().collect();
    jobs.sort_by_key(|j| (std::cmp::Reverse(j.priority), j.created_at, j.id));
    jobs
}

/// Jobs to start now, in start order.
///
/// `running` holds the session of every running job: a session runs one job
/// at a time. `free_ram` is `None` when unknown, which never blocks.
pub fn plan<'a>(
    queued: impl IntoIterator<Item = &'a Job>,
    running: &[SessionId],
    free_ram: Option<u64>,
    limits: Limits,
) -> Vec<JobId> {
    if free_ram.is_some_and(|free| free < limits.min_free_ram) {
        return Vec::new();
    }
    let mut busy: Vec<SessionId> = running.to_vec();
    let mut picks = Vec::new();
    for job in run_order(queued) {
        if busy.len() >= limits.max_concurrent {
            break;
        }
        if busy.contains(&job.session_id) {
            continue;
        }
        busy.push(job.session_id);
        picks.push(job.id);
    }
    picks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::JobStatus;

    const GB: u64 = 1 << 30;

    fn limits(max: usize) -> Limits {
        Limits {
            max_concurrent: max,
            min_free_ram: GB,
        }
    }

    fn job(id: i64, session: SessionId, priority: i32, created_at: i64) -> Job {
        Job {
            id: JobId(id),
            session_id: session,
            prompt_id: None,
            prompt: "p".into(),
            model: "sonnet".into(),
            priority,
            fork: false,
            status: JobStatus::Queued,
            created_at,
        }
    }

    fn ids(v: Vec<JobId>) -> Vec<i64> {
        v.into_iter().map(|j| j.0).collect()
    }

    #[test]
    fn oldest_first_at_equal_priority() {
        let (a, b) = (SessionId::random(), SessionId::random());
        let q = [job(2, a, 0, 20), job(1, b, 0, 10)];
        assert_eq!(ids(plan(&q, &[], None, limits(1))), [1]);
    }

    #[test]
    fn higher_priority_jumps_the_queue() {
        let (a, b, c) = (
            SessionId::random(),
            SessionId::random(),
            SessionId::random(),
        );
        let q = [job(1, a, 0, 10), job(2, b, 5, 30), job(3, c, -1, 5)];
        assert_eq!(ids(plan(&q, &[], None, limits(3))), [2, 1, 3]);
    }

    #[test]
    fn same_timestamp_breaks_ties_by_id() {
        let (a, b) = (SessionId::random(), SessionId::random());
        let q = [job(7, a, 0, 10), job(3, b, 0, 10)];
        assert_eq!(ids(plan(&q, &[], None, limits(1))), [3]);
    }

    #[test]
    fn one_job_per_session() {
        let (a, b) = (SessionId::random(), SessionId::random());
        let q = [job(1, a, 0, 1), job(2, a, 0, 2), job(3, b, 0, 3)];
        assert_eq!(ids(plan(&q, &[], None, limits(3))), [1, 3]);
    }

    #[test]
    fn busy_session_is_skipped_not_blocking() {
        let (a, b) = (SessionId::random(), SessionId::random());
        let q = [job(1, a, 9, 1), job(2, b, 0, 2)];
        assert_eq!(ids(plan(&q, &[a], None, limits(3))), [2]);
    }

    #[test]
    fn never_exceeds_max_concurrent() {
        let s: Vec<SessionId> = (0..4).map(|_| SessionId::random()).collect();
        let q: Vec<Job> = (0..4).map(|i| job(i, s[i as usize], 0, i)).collect();
        assert_eq!(ids(plan(&q, &[], None, limits(2))), [0, 1]);
        let other = SessionId::random();
        assert_eq!(ids(plan(&q, &[other], None, limits(2))), [0]);
        assert!(plan(&q, &[other, SessionId::random()], None, limits(2)).is_empty());
        assert!(plan(&q, &[], None, limits(0)).is_empty());
    }

    #[test]
    fn low_ram_starts_nothing() {
        let q = [job(1, SessionId::random(), 0, 1)];
        assert!(plan(&q, &[], Some(GB - 1), limits(3)).is_empty());
        assert_eq!(ids(plan(&q, &[], Some(GB), limits(3))), [1]);
    }

    #[test]
    fn unknown_ram_does_not_block() {
        let q = [job(1, SessionId::random(), 0, 1)];
        assert_eq!(ids(plan(&q, &[], None, limits(3))), [1]);
    }

    #[test]
    fn empty_queue_plans_nothing() {
        assert!(plan(&[], &[], Some(10 * GB), limits(3)).is_empty());
    }

    #[test]
    fn run_order_lists_everything() {
        let a = SessionId::random();
        let q = [job(1, a, 0, 1), job(2, a, 1, 2), job(3, a, 0, 0)];
        let order: Vec<i64> = run_order(&q).iter().map(|j| j.id.0).collect();
        assert_eq!(order, [2, 3, 1]);
    }
}
