//! Spreads out the starts of `claude`.
//!
//! Many `claude` processes starting in the same instant all find the same
//! expiring OAuth token and all try to refresh it; one refresh invalidates
//! the others and the session asks to log in again. Starts that are a short
//! while apart share the refreshed token instead.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// Least time between two starts, and the most that is added at random.
const GAP: Duration = Duration::from_millis(350);
const JITTER: Duration = Duration::from_millis(150);

/// Hands out start times that are at least `gap` apart.
#[derive(Debug)]
pub struct Stagger {
    gap: Duration,
    jitter: Duration,
    next: Mutex<Option<Instant>>,
}

impl Stagger {
    pub const fn new(gap: Duration, jitter: Duration) -> Stagger {
        Stagger {
            gap,
            jitter,
            next: Mutex::new(None),
        }
    }

    /// The one shared by every `claude` start in this process.
    pub fn shared() -> Arc<Stagger> {
        static SHARED: OnceLock<Arc<Stagger>> = OnceLock::new();
        SHARED
            .get_or_init(|| Arc::new(Stagger::new(GAP, JITTER)))
            .clone()
    }

    /// Takes the next free start time and returns how long from `now` to it.
    /// `extra` (at most the jitter) is added to the gap that follows it.
    pub fn reserve(&self, now: Instant, extra: Duration) -> Duration {
        let extra = extra.min(self.jitter);
        let mut next = self.next.lock().unwrap_or_else(PoisonError::into_inner);
        let start = next.map_or(now, |at| at.max(now));
        *next = Some(start + self.gap + extra);
        start.saturating_duration_since(now)
    }

    /// Waits for this start's turn.
    pub async fn wait(&self) {
        let wait = self.reserve(Instant::now(), self.random_extra());
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    fn random_extra(&self) -> Duration {
        random_below(self.jitter)
    }
}

/// A random duration in `[0, max)`.
pub fn random_below(max: Duration) -> Duration {
    let micros = u64::try_from(max.as_micros()).unwrap_or(u64::MAX);
    if micros == 0 {
        return Duration::ZERO;
    }
    let roll = RandomState::new().build_hasher().finish();
    Duration::from_micros(roll % micros)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[test]
    fn the_first_start_does_not_wait_and_the_rest_queue_up() {
        let s = Stagger::new(MS(100), MS(50));
        let t0 = Instant::now();
        assert_eq!(s.reserve(t0, MS(0)), MS(0));
        assert_eq!(s.reserve(t0, MS(20)), MS(100));
        assert_eq!(s.reserve(t0, MS(0)), MS(220));
        assert_eq!(s.reserve(t0, MS(0)), MS(320));
    }

    #[test]
    fn a_start_after_a_quiet_spell_does_not_wait() {
        let s = Stagger::new(MS(100), MS(50));
        let t0 = Instant::now();
        s.reserve(t0, MS(0));
        assert_eq!(s.reserve(t0 + MS(500), MS(0)), MS(0));
    }

    #[test]
    fn the_random_part_is_capped_by_the_jitter() {
        let s = Stagger::new(MS(100), MS(50));
        let t0 = Instant::now();
        s.reserve(t0, MS(10_000));
        assert_eq!(s.reserve(t0, MS(0)), MS(150));
        for _ in 0..50 {
            assert!(s.random_extra() < MS(50));
        }
    }
}
