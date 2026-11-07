use super::*;

pub(super) fn fu(kind: FollowKind, text: &str, paths: &[&str], job: i64) -> FollowUp {
    let mut f = FollowUp::new(
        kind,
        text,
        Source {
            job: Some(JobId(job)),
            ..Source::default()
        },
        1000 + job,
    );
    f.paths = paths.iter().map(|p| p.to_string()).collect();
    f
}

#[test]
fn the_same_words_with_other_punctuation_are_one_follow_up() {
    let a = fu(
        FollowKind::SuggestedTask,
        "Add a retry to the fetch client.",
        &["src/net.rs"],
        1,
    );
    let b = fu(
        FollowKind::SuggestedTask,
        "add a RETRY to the fetch-client",
        &["./src/net.rs"],
        2,
    );
    assert!(a.same_as(&b));
    let known = vec![a];
    assert_eq!(find_duplicate(&known, &b), Some(0));
}

#[test]
fn another_kind_or_other_files_is_another_follow_up() {
    let a = fu(FollowKind::SuggestedTask, "add a retry", &["a.rs"], 1);
    assert!(!a.same_as(&fu(FollowKind::Risk, "add a retry", &["a.rs"], 2)));
    assert!(!a.same_as(&fu(FollowKind::SuggestedTask, "add a retry", &["b.rs"], 2)));
    assert!(!a.same_as(&fu(
        FollowKind::SuggestedTask,
        "rename the module",
        &["a.rs"],
        2
    )));
}

#[test]
fn a_finished_follow_up_is_not_matched_but_a_declined_one_is() {
    let mut done = fu(FollowKind::Risk, "x breaks on empty input", &[], 1);
    done.status = FollowStatus::Done;
    let fresh = fu(FollowKind::Risk, "x breaks on empty input", &[], 2);
    assert_eq!(find_duplicate(std::slice::from_ref(&done), &fresh), None);
    done.status = FollowStatus::Declined;
    assert_eq!(find_duplicate(&[done], &fresh), Some(0));
}

#[test]
fn merging_joins_sources_and_keeps_the_stronger_signal() {
    let mut a = fu(FollowKind::Risk, "x breaks on empty input", &["a.rs"], 1);
    let mut b = fu(FollowKind::Risk, "x breaks on empty input", &["a.rs"], 2);
    b.urgency = Urgency::High;
    b.rationale = "a longer explanation".into();
    b.est_cost_usd = Some(0.5);
    a.merge(&b);
    assert_eq!(a.jobs(), vec![JobId(1), JobId(2)]);
    assert_eq!(a.urgency, Urgency::High);
    assert_eq!(a.rationale, "a longer explanation");
    assert_eq!(a.est_cost_usd, Some(0.5));
    assert_eq!(a.created, 1001);
    a.merge(&b);
    assert_eq!(a.sources.len(), 2, "no source twice");
}

#[test]
fn urgent_is_a_p1_risk_or_a_blocker_on_running_work() {
    let mut risk = fu(FollowKind::Risk, "leaks a key", &[], 1);
    assert!(!risk.urgent(true));
    risk.urgency = Urgency::High;
    assert!(risk.urgent(false));
    let blocker = fu(FollowKind::Blocker, "needs a migration", &[], 1);
    assert!(blocker.urgent(true));
    assert!(!blocker.urgent(false));
    assert!(!fu(FollowKind::SuggestedTask, "x", &[], 1).urgent(true));
}

#[test]
fn deferred_waits_until_due_and_parked_never() {
    let mut f = fu(FollowKind::SuggestedTask, "x", &[], 1);
    f.status = FollowStatus::Deferred;
    f.due_at = Some(500);
    assert!(!f.waiting(499));
    assert!(f.waiting(500));
    f.due_at = None;
    assert!(!f.waiting(i64::MAX));
}

#[test]
fn names_round_trip() {
    for k in FollowKind::ALL {
        assert_eq!(FollowKind::parse(k.as_str()), Some(k));
    }
    for s in FollowStatus::ALL {
        assert_eq!(FollowStatus::parse(s.as_str()), Some(s));
    }
    assert_eq!(
        FollowKind::parse("suggested_task"),
        Some(FollowKind::SuggestedTask)
    );
    assert_eq!(Urgency::parse("P1"), Some(Urgency::High));
    let by = DecidedBy::Order("docs".into());
    assert_eq!(DecidedBy::parse(&by.to_text()), Some(by));
    assert_eq!(DecidedBy::parse("owner"), Some(DecidedBy::Owner));
    assert_eq!(Source::default().line(), "—");
}
