//! Failures caused by several `claude` processes writing its config at once.
//!
//! Claude Code keeps its state in one `.claude.json`. Sessions that start
//! together can find it half written, locked, or mid-rename, and exit at
//! once with an error about that file. Nothing has run by then, so the
//! start is simply tried again a little later.

use std::time::Duration;

/// How many times a start is repeated after such a failure.
pub const RETRIES: u32 = 3;

/// Wait before retry `attempt` (0-based): 250 ms, 500 ms, 1 s, plus up to
/// `spread` so the sessions that collided do not collide again.
pub fn backoff(attempt: u32, spread: Duration) -> Duration {
    Duration::from_millis(250 << attempt.min(6)) + spread
}

/// What in stderr points at the config file being under contention.
const SYMPTOMS: [&str; 11] = [
    "corrupt",
    "ebusy",
    "eperm",
    "eacces",
    "enoent",
    "unexpected end of json",
    "unexpected token",
    "json parse",
    "being used by another process",
    "sharing violation",
    "resource busy",
];

/// Whether `stderr` says the config file could not be read or replaced.
pub fn is_config_contention(stderr: &str) -> bool {
    let text = stderr.to_ascii_lowercase();
    text.contains(".claude.json") && SYMPTOMS.iter().any(|s| text.contains(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_known_failures_are_recognised() {
        for text in [
            "Claude configuration file at C:\\Users\\a\\.claude.json is corrupted: Unexpected end of JSON input",
            "Error: EBUSY: resource busy or locked, rename 'C:\\Users\\a\\.claude.json.tmp.123' -> 'C:\\Users\\a\\.claude.json'",
            "EPERM: operation not permitted, rename '/home/a/.claude.json.tmp' -> '/home/a/.claude.json'",
            "The process cannot access the file .claude.json because it is being used by another process.",
        ] {
            assert!(is_config_contention(text), "{text}");
        }
    }

    #[test]
    fn other_failures_are_left_alone() {
        for text in [
            "",
            "Invalid API key",
            "error: unexpected argument '--bogus' found",
            "EPERM: operation not permitted, open 'C:\\secret.txt'",
            "corrupt transcript",
        ] {
            assert!(!is_config_contention(text), "{text}");
        }
    }

    #[test]
    fn waits_grow_and_stay_bounded() {
        let none = Duration::ZERO;
        assert_eq!(backoff(0, none), Duration::from_millis(250));
        assert_eq!(backoff(1, none), Duration::from_millis(500));
        assert_eq!(backoff(2, none), Duration::from_secs(1));
        assert!(backoff(60, none) <= Duration::from_secs(16));
        assert_eq!(
            backoff(0, Duration::from_millis(40)),
            Duration::from_millis(290)
        );
    }
}
