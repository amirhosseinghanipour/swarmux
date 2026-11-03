use super::*;
use crate::telemetry::{DAY, RateLimit};

/// A small deterministic generator for randomized property checks.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next() % items.len() as u64) as usize]
    }
}

fn reading(window: &str, used: f64, resets_at: Millis) -> RateLimit {
    RateLimit {
        window: window.into(),
        status: if used >= 1.0 { "rejected" } else { "allowed" }.into(),
        utilization: Some(used),
        resets_at: Some(resets_at),
        surpassed_threshold: None,
    }
}

fn state(window: &str, used: f64, resets_at: Millis, at: Millis) -> LimitState {
    LimitState::new(reading(window, used, resets_at), at)
}

fn policy() -> GovernorPolicy {
    GovernorPolicy::default()
}

// --- reservations -------------------------------------------------------

#[test]
fn shares_split_the_reservation() {
    let r = Reservation::new(10.0);
    let total: f64 = MissionPhase::SHARES.iter().map(|(p, _)| r.share(*p)).sum();
    assert!((total - 10.0).abs() < 1e-9);
    assert!((r.share(MissionPhase::Verify) - 2.0).abs() < 1e-9);
    assert_eq!(Reservation::new(-3.0).total, 0.0);
    assert_eq!(Reservation::new(f64::NAN).total, 0.0);
}

#[test]
fn implementation_gets_unspent_planning_but_never_the_verify_share() {
    let r = Reservation::new(10.0);
    let spent = Spend {
        plan: 0.4,
        ..Spend::default()
    };
    // Plan 1.0 + implement 6.0, less 0.4 planned.
    assert!((r.headroom(MissionPhase::Implement, &spent) - 6.6).abs() < 1e-9);
    let overspent = Spend {
        implement: 50.0,
        ..Spend::default()
    };
    assert_eq!(r.headroom(MissionPhase::Implement, &overspent), 0.0);
    assert!((r.headroom(MissionPhase::Verify, &overspent) - 2.0).abs() < 1e-9);
    assert!((r.headroom(MissionPhase::Review, &overspent) - 1.0).abs() < 1e-9);
}

#[test]
fn property_verify_is_never_starved_by_the_implementation() {
    let mut rng = Rng(0x5eed_0001);
    for _ in 0..10_000 {
        let r = Reservation::new(rng.range(0.0, 100.0));
        let spent = Spend {
            plan: rng.range(0.0, 30.0),
            implement: rng.range(0.0, 200.0),
            verify: 0.0,
            review: 0.0,
        };
        let verify = r.headroom(MissionPhase::Verify, &spent);
        assert!((verify - r.share(MissionPhase::Verify)).abs() < 1e-9);
        // Whatever the implementation may still spend leaves verify and
        // review whole.
        let implement = r.headroom(MissionPhase::Implement, &spent);
        let committed = spent.plan + spent.implement + implement;
        let held = r.share(MissionPhase::Verify) + r.share(MissionPhase::Review);
        assert!(implement >= 0.0);
        assert!(
            implement == 0.0 || committed <= r.total - held + 1e-9,
            "{r:?} {spent:?} {implement}"
        );
    }
}

// --- forecasts ----------------------------------------------------------

#[test]
fn window_labels() {
    assert_eq!(window_label("five_hour"), "5h");
    assert_eq!(window_label("seven_day"), "7d");
    assert_eq!(window_label("seven_day_opus"), "7d opus");
    assert_eq!(window_label("monthly_spend"), "monthly spend");
}

#[test]
fn forecast_follows_the_recent_pace() {
    // 10% per hour over the last two hours.
    let mut s = state("five_hour", 0.2, 5 * HOUR, HOUR);
    s.update(reading("five_hour", 0.3, 5 * HOUR), 2 * HOUR);
    s.update(reading("five_hour", 0.4, 5 * HOUR), 3 * HOUR);
    let f = forecast(&s, 3 * HOUR + 30 * MINUTE).unwrap();
    assert!((f - 0.45).abs() < 1e-9, "{f}");
    // The past is the reading itself.
    assert_eq!(forecast(&s, HOUR), Some(0.4));
    // Never past full.
    let mut steep = state("seven_day", 0.5, 7 * DAY, HOUR);
    steep.update(reading("seven_day", 0.9, 7 * DAY), 2 * HOUR);
    assert_eq!(forecast(&steep, 5 * HOUR), Some(1.0));
}

#[test]
fn forecast_starts_the_window_over_after_the_reset() {
    let mut s = state("five_hour", 0.0, 5 * HOUR, 0);
    s.update(reading("five_hour", 0.5, 5 * HOUR), 5 * HOUR / 2);
    // 20% per hour; an hour after the reset the new window is at 20%.
    let f = forecast(&s, 6 * HOUR).unwrap();
    assert!((f - 0.2).abs() < 1e-6, "{f}");
}

#[test]
fn forecast_without_a_pace_stays_flat() {
    let s = LimitState::new(
        RateLimit {
            window: "monthly".into(),
            status: "allowed".into(),
            utilization: Some(0.3),
            resets_at: None,
            surpassed_threshold: None,
        },
        100,
    );
    assert_eq!(forecast(&s, 100 + DAY), Some(0.3));
}

/// Usage of a week: work hours on weekdays, little at night and on the
/// weekend, every hour noisy. Utilization every 15 minutes.
fn synthetic_week(seed: u64) -> Vec<(Millis, f64)> {
    let mut rng = Rng(seed);
    let mut used = 0.0;
    let mut out = vec![(0, 0.0)];
    let step = 15 * MINUTE;
    let mut t = 0;
    while t < 7 * DAY {
        let hour = (t % DAY) / HOUR;
        let day = t / DAY;
        let weekend = day >= 5;
        let base = match hour {
            9..=12 | 14..=18 => 0.015,
            19..=22 => 0.006,
            _ => 0.0005,
        };
        let day_factor = if weekend { 0.6 } else { 1.0 };
        used += base * day_factor * rng.range(0.6, 1.4) * (step as f64 / HOUR as f64);
        t += step;
        out.push((t, used));
    }
    out
}

/// Mean and worst absolute error of 24-hour forecasts of the weekly window,
/// made every hour from the second day on.
fn day_ahead_errors(week: &[(Millis, f64)]) -> (f64, f64) {
    let reset = 7 * DAY;
    let at = |t: Millis| {
        week.iter()
            .rev()
            .find(|(at, _)| *at <= t)
            .map_or(0.0, |(_, u)| *u)
    };
    let mut errors = Vec::new();
    let mut now = DAY;
    while now + DAY < reset {
        let mut s: Option<LimitState> = None;
        for &(t, u) in week.iter().filter(|(t, _)| *t <= now) {
            let r = reading("seven_day", u, reset);
            match s.as_mut() {
                Some(s) => s.update(r, t),
                None => s = Some(LimitState::new(r, t)),
            }
        }
        let Some(s) = s else { break };
        let predicted = forecast(&s, now + DAY).unwrap_or(0.0);
        errors.push((predicted - at(now + DAY)).abs());
        now += HOUR;
    }
    let mean = errors.iter().sum::<f64>() / errors.len() as f64;
    let worst = errors.iter().copied().fold(0.0, f64::max);
    (mean, worst)
}

#[test]
fn day_ahead_forecast_error_is_within_ten_points() {
    for seed in [1, 7, 42, 1234, 99_999] {
        let week = synthetic_week(seed);
        let (mean, worst) = day_ahead_errors(&week);
        assert!(mean <= 0.05, "seed {seed}: mean {mean:.4}");
        assert!(worst <= 0.10, "seed {seed}: worst {worst:.4}");
    }
}

// --- admission ----------------------------------------------------------

#[test]
fn hold_threshold_defers_new_work_until_the_reset() {
    let now = HOUR;
    let w = [state("five_hour", 0.97, 5 * HOUR, now)];
    let a = admit(&policy(), &w, "sonnet", JobClass::Work, now);
    assert_eq!(
        a,
        Admission::Defer {
            window: "five_hour".into(),
            used: 0.97,
            until: Some(5 * HOUR)
        }
    );
    assert_eq!(
        a.reason(0).as_deref(),
        Some("deferred: 5h at 97%, resumes 05:00")
    );
    // Once the reset passes the reading is stale and the job goes.
    assert_eq!(
        admit(&policy(), &w, "sonnet", JobClass::Work, 5 * HOUR),
        Admission::Admit
    );
    // Verification keeps going until the window is full.
    assert_eq!(
        admit(&policy(), &w, "sonnet", JobClass::Verify, now),
        Admission::Admit
    );
}

#[test]
fn the_later_reset_decides_when_two_windows_hold() {
    let now = HOUR;
    let w = [
        state("five_hour", 0.99, 3 * HOUR, now),
        state("seven_day", 0.98, 2 * DAY, now),
    ];
    match admit(&policy(), &w, "sonnet", JobClass::Work, now) {
        Admission::Defer { window, until, .. } => {
            assert_eq!((window.as_str(), until), ("seven_day", Some(2 * DAY)));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn rejected_window_defers_even_verification() {
    let mut r = reading("five_hour", 0.5, 5 * HOUR);
    r.status = "rejected".into();
    let w = [LimitState::new(r, HOUR)];
    assert!(admit(&policy(), &w, "haiku", JobClass::Verify, HOUR).is_deferred());
}

#[test]
fn pricey_models_are_downgraded_near_the_limit() {
    let now = HOUR;
    let w = [state("seven_day", 0.92, 3 * DAY, now)];
    let a = admit(&policy(), &w, "opus", JobClass::Work, now);
    let Admission::Downgrade {
        from,
        to,
        window,
        used,
        ahead,
    } = &a
    else {
        panic!("{a:?}");
    };
    assert_eq!((from.as_str(), to.as_str()), ("opus", "sonnet"));
    assert_eq!((window.as_str(), *used), ("seven_day", 0.92));
    assert!(*ahead >= 0.92 && *ahead < 0.93, "{ahead}");
    assert_eq!(a.reason(0).as_deref(), Some("opus → sonnet: 7d at 92%"));
    let rising = Admission::Downgrade {
        from: "claude-opus-5".into(),
        to: "sonnet".into(),
        window: "five_hour".into(),
        used: 0.8,
        ahead: 0.98,
    };
    assert_eq!(
        rising.reason(0).as_deref(),
        Some("opus → sonnet: 5h at 80%, heading for 98%")
    );
    // Already on sonnet (or cheaper), or an unknown model: admitted as is.
    for model in ["sonnet", "claude-haiku-4", "mystery"] {
        assert_eq!(
            admit(&policy(), &w, model, JobClass::Work, now),
            Admission::Admit
        );
    }
    assert!(matches!(
        admit(&policy(), &w, "fable", JobClass::Work, now),
        Admission::Downgrade { .. }
    ));
}

#[test]
fn a_fast_burn_downgrades_before_the_threshold() {
    // 80% and climbing 20 points an hour: 90% within the half hour, past
    // the hold threshold soon after.
    let mut s = state("five_hour", 0.6, 5 * HOUR, HOUR);
    s.update(reading("five_hour", 0.8, 5 * HOUR), 2 * HOUR);
    let p = GovernorPolicy {
        horizon: HOUR,
        ..policy()
    };
    assert!(matches!(
        admit(&p, &[s], "opus", JobClass::Work, 2 * HOUR),
        Admission::Downgrade { .. }
    ));
}

#[test]
fn unknown_windows_admit() {
    assert_eq!(
        admit(&policy(), &[], "opus", JobClass::Work, 0),
        Admission::Admit
    );
    let mut r = reading("five_hour", 0.0, 5 * HOUR);
    r.utilization = None;
    let w = [LimitState::new(r, 0)];
    assert_eq!(
        admit(&policy(), &w, "opus", JobClass::Work, 0),
        Admission::Admit
    );
}

#[test]
fn defer_reason_without_a_reset_time() {
    let a = Admission::Defer {
        window: "seven_day_opus".into(),
        used: 0.981,
        until: None,
    };
    assert_eq!(
        a.reason(0).as_deref(),
        Some("deferred: 7d opus at 98%, resumes after the reset")
    );
    assert_eq!(Admission::Admit.reason(0), None);
}

fn random_windows(rng: &mut Rng, now: Millis) -> Vec<LimitState> {
    let names = ["five_hour", "seven_day", "seven_day_opus"];
    let n = rng.next() % 4;
    (0..n)
        .map(|_| {
            let name = *rng.pick(&names);
            let used = rng.range(0.0, 1.1);
            // Resets from an hour ago to three days ahead.
            let reset = now + (rng.range(-1.0, 72.0) * HOUR as f64) as Millis;
            let mut s = state(name, (used - 0.1).max(0.0), reset, now - HOUR);
            s.update(reading(name, used, reset), now);
            s
        })
        .collect()
}

#[test]
fn property_admission_respects_the_hold_threshold() {
    let mut rng = Rng(0xad_0157);
    let models = ["haiku", "sonnet", "opus", "fable", "other"];
    for _ in 0..20_000 {
        let now = 10 * DAY;
        let windows = random_windows(&mut rng, now);
        let p = GovernorPolicy {
            hold_at: rng.range(0.5, 1.0),
            downgrade_at: rng.range(0.3, 1.0),
            downgrade_to: (*rng.pick(&["haiku", "sonnet"])).into(),
            horizon: (rng.range(0.0, 3.0) * HOUR as f64) as Millis,
            load_hold_at: 1.0,
        };
        let model = *rng.pick(&models);
        let class = if rng.next().is_multiple_of(4) {
            JobClass::Verify
        } else {
            JobClass::Work
        };
        let a = admit(&p, &windows, model, class, now);
        let live: Vec<(&LimitState, f64)> = windows
            .iter()
            .filter(|w| !w.is_stale(now))
            .filter_map(|w| w.limit.utilization.map(|u| (w, u)))
            .collect();
        let limit = if class == JobClass::Work {
            p.hold_at
        } else {
            1.0
        };
        let must_hold = live.iter().any(|(_, u)| *u >= limit);
        match &a {
            Admission::Defer { until, used, .. } => {
                assert!(must_hold, "deferred without cause: {a:?}");
                assert!(*used >= limit);
                // Deferred work resumes at a future reset of a live window.
                let until = until.expect("random windows have resets");
                assert!(until > now);
                assert!(live.iter().any(|(w, _)| w.limit.resets_at == Some(until)));
            }
            Admission::Downgrade { from, to, .. } => {
                assert!(!must_hold, "{a:?}");
                assert_eq!(class, JobClass::Work);
                assert!(tier(from) > tier(to), "{from} → {to}");
                assert!(tier(to).is_some());
            }
            Admission::Admit => assert!(!must_hold, "admitted at the limit: {windows:?}"),
            Admission::Hold { .. } => unreachable!("`admit` knows nothing of load"),
        }
        // Stale readings never hold anything back.
        let after = windows
            .iter()
            .filter_map(|w| w.limit.resets_at)
            .max()
            .map_or(now, |r| r.max(now));
        assert!(!admit(&p, &windows, model, class, after).is_deferred());
    }
}

#[test]
fn property_lower_use_never_admits_less() {
    let mut rng = Rng(0x0dd_ba11);
    let rank = |a: &Admission| match a {
        Admission::Admit => 2,
        Admission::Downgrade { .. } => 1,
        Admission::Defer { .. } | Admission::Hold { .. } => 0,
    };
    for _ in 0..10_000 {
        let now = DAY;
        let used = rng.range(0.0, 1.0);
        let lower = used * rng.unit();
        let reset = now + (rng.range(0.1, 5.0) * HOUR as f64) as Millis;
        let high = [state("five_hour", used, reset, now)];
        let low = [state("five_hour", lower, reset, now)];
        let p = policy();
        for model in ["opus", "sonnet"] {
            let a = admit(&p, &high, model, JobClass::Work, now);
            let b = admit(&p, &low, model, JobClass::Work, now);
            assert!(rank(&b) >= rank(&a), "{used} → {a:?}, {lower} → {b:?}");
        }
    }
}

#[test]
fn limit_errors() {
    assert!(is_limit_error("Claude AI usage limit reached|1793786400"));
    assert!(is_limit_error("API Error: Rate limit exceeded"));
    assert!(is_limit_error("stop_failure: rate_limit"));
    assert!(!is_limit_error("claude exited with code 1: boom"));
}

// --- power and the budget line -----------------------------------------

#[test]
fn battery_caps_concurrency() {
    let base = Limits {
        max_concurrent: 6,
        min_free_ram: 1,
    };
    assert_eq!(power_limits(base, true, 2).max_concurrent, 2);
    assert_eq!(power_limits(base, false, 2).max_concurrent, 6);
    let one = Limits {
        max_concurrent: 1,
        ..base
    };
    assert_eq!(power_limits(one, true, 2).max_concurrent, 1);
    assert_eq!(power_limits(base, true, 0).max_concurrent, 1);
}

#[test]
fn budget_line_names_the_cap_and_the_windows() {
    let now = HOUR;
    let w = [
        state("seven_day", 0.91, 3 * DAY, now),
        state("five_hour", 0.62, 4 * HOUR, now),
    ];
    assert_eq!(
        budget_line(&w, Some(2.4), 0.97, now, 0).as_deref(),
        Some(
            "[budget] up to $2.40 for this run · 5h 62%, resets 04:00 · 7d 91% · new work holds at 97%"
        )
    );
    assert_eq!(
        budget_line(&[], Some(1.0), 0.97, now, 0).as_deref(),
        Some("[budget] up to $1.00 for this run")
    );
    assert_eq!(budget_line(&[], None, 0.97, now, 0), None);
    // A reset window says nothing.
    assert_eq!(budget_line(&w, None, 0.97, 4 * DAY, 0), None);
}

#[test]
fn the_load_hold_only_touches_new_work_the_windows_let_through() {
    let p = GovernorPolicy::default();
    let hold = |a, class, load| hold_for_load(&p, a, class, load);
    assert_eq!(
        hold(Admission::Admit, JobClass::Work, Some(0.78)),
        Admission::Hold {
            load: 0.78,
            at: 0.70
        }
    );
    assert_eq!(
        hold(Admission::Admit, JobClass::Work, Some(0.78))
            .reason(0)
            .as_deref(),
        Some("held: you're at 78% load")
    );
    assert_eq!(
        hold(Admission::Admit, JobClass::Work, Some(0.70)),
        Admission::Admit
    );
    assert_eq!(
        hold(Admission::Admit, JobClass::Work, None),
        Admission::Admit
    );
    assert_eq!(
        hold(Admission::Admit, JobClass::Verify, Some(0.99)),
        Admission::Admit
    );
    assert_eq!(
        hold(Admission::Admit, JobClass::Work, Some(f64::NAN)),
        Admission::Admit
    );
    let defer = Admission::Defer {
        window: "five_hour".into(),
        used: 0.99,
        until: None,
    };
    assert_eq!(hold(defer.clone(), JobClass::Work, Some(0.99)), defer);
    assert!(Admission::Hold { load: 0.8, at: 0.7 }.is_held());
}
