//! The service lock (`daemon::lock`), in a test binary of its own.
//!
//! The lock is a `flock`, which belongs to the open file description. A
//! thread of the same process that starts a child (`git`, a shell, the hook)
//! at the moment the lock is dropped gives the child a copy of that
//! description until it execs, so the lock still looks held. Measured: with
//! four threads starting `true` in a loop, 20 of 300 drops were followed by
//! `is_held` = true; with none, 0 of 300. In the library's unit-test binary,
//! where many tests start `git`, that made the first test below fail now and
//! then. Here no test starts a process, so the race cannot happen. The tests
//! are the ones that were in `lock.rs`, word for word.

use swarmux_infrastructure::daemon::{Holder, LockError, ServiceLock, holder, is_held};

#[test]
fn a_second_holder_is_refused_and_told_who_holds_it() {
    let dir = tempfile::tempdir().unwrap();
    let first = ServiceLock::acquire(dir.path(), "0.1.0", 1_000).unwrap();
    match ServiceLock::acquire(dir.path(), "0.1.0", 2_000) {
        Err(LockError::Held(h)) => {
            assert_eq!(h.pid, Some(std::process::id()));
            assert_eq!(h.version.as_deref(), Some("0.1.0"));
            assert_eq!(h.started_at, Some(1_000));
        }
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("two holders"),
    }
    assert!(is_held(dir.path()));
    drop(first);
    assert!(!is_held(dir.path()));
    assert_eq!(holder(dir.path()), Holder::default(), "the pid file goes");
    assert!(ServiceLock::acquire(dir.path(), "0.1.0", 3_000).is_ok());
}

#[test]
fn a_crashed_runs_pid_file_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("daemon.pid"), "4294967\n0.0.9\n5\n").unwrap();
    std::fs::write(dir.path().join("daemon.lock"), "").unwrap();
    let lock = ServiceLock::acquire(dir.path(), "0.1.0", 9).unwrap();
    let h = holder(dir.path());
    assert_eq!(h.pid, Some(std::process::id()));
    assert_eq!(h.version.as_deref(), Some("0.1.0"));
    drop(lock);
}

#[test]
fn a_damaged_pid_file_reads_as_unknown() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("daemon.pid"), "not a pid\n").unwrap();
    let h = holder(dir.path());
    assert_eq!(h.pid, None);
    assert_eq!(h.version, None);
}
