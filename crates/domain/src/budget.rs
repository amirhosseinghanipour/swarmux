//! Budgets and the usage governor's rules: reservations split into phase
//! shares, forecasts of the subscription windows, and whether a queued job
//! may start now, later, or on a cheaper model.

use crate::mission::{MissionPhase, Spend};
use crate::telemetry::{HOUR, LimitState, MINUTE, local_clock, window_length};
use crate::{Family, Limits, Millis};

/// A budget reserved up front and split into phase shares.
///
/// Planning left over goes to the implementation; verification and review
/// keep their own shares, which the implementation can never spend.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reservation {
    pub total: f64,
}

impl Reservation {
    /// A reservation of `usd` (negative or not a number reserves nothing).
    pub fn new(usd: f64) -> Reservation {
        Reservation {
            total: if usd.is_finite() { usd.max(0.0) } else { 0.0 },
        }
    }

    /// The share of `phase`, in dollars.
    pub fn share(&self, phase: MissionPhase) -> f64 {
        MissionPhase::SHARES
            .iter()
            .find(|(p, _)| *p == phase)
            .map_or(0.0, |(_, s)| self.total * s)
    }

    /// What `phase` may still spend after `spent`.
    pub fn headroom(&self, phase: MissionPhase, spent: &Spend) -> f64 {
        let left = match phase {
            MissionPhase::Plan => self.share(MissionPhase::Plan) - spent.plan,
            MissionPhase::Implement => {
                self.share(MissionPhase::Plan) + self.share(MissionPhase::Implement)
                    - spent.plan
                    - spent.implement
            }
            MissionPhase::Verify => self.share(MissionPhase::Verify) - spent.verify,
            MissionPhase::Review => self.share(MissionPhase::Review) - spent.review,
        };
        if left.is_finite() { left.max(0.0) } else { 0.0 }
    }
}

/// Readings closer together than this give no pace.
const MIN_SPAN: Millis = 5 * MINUTE;
/// The shortest and longest stretch of readings a pace is taken over.
const MIN_LOOKBACK: Millis = HOUR;
const MAX_LOOKBACK: Millis = 24 * HOUR;

/// Share of the window used per millisecond, judged over the readings of
/// the last `lookback` (one to 24 hours). When those readings span less
/// than half of it, the average since the window opened is used instead.
pub fn pace(state: &LimitState, lookback: Millis) -> Option<f64> {
    let lookback = lookback.clamp(MIN_LOOKBACK, MAX_LOOKBACK);
    let &(last_at, last) = state.samples.last()?;
    let trailing = state
        .samples
        .iter()
        .find(|(at, _)| last_at - at <= lookback)
        .filter(|(at, _)| last_at - at >= MIN_SPAN)
        .map(|&(at, u)| (last_at - at, ((last - u) / (last_at - at) as f64).max(0.0)));
    let average = state
        .limit
        .resets_at
        .zip(window_length(&state.limit.window))
        .map(|(reset, len)| reset - len)
        .filter(|opened| last_at > *opened)
        .map(|opened| (last / (last_at - opened) as f64).max(0.0));
    match (trailing, average) {
        (Some((span, p)), _) if span * 2 >= lookback => Some(p),
        (t, Some(avg)) => Some(t.map_or(avg, |(_, p)| p.max(avg))),
        (t, None) => t.map(|(_, p)| p),
    }
}

/// The window's utilization (0 to 1) at `at` if usage keeps its pace. Past
/// the reset the window starts over at the same pace.
pub fn forecast(state: &LimitState, at: Millis) -> Option<f64> {
    let (last_at, last) = state
        .samples
        .last()
        .copied()
        .or_else(|| Some((state.observed_at, state.limit.utilization?)))?;
    if at <= last_at {
        return Some(last.clamp(0.0, 1.0));
    }
    let p = pace(state, at - last_at).unwrap_or(0.0);
    let used = match state.limit.resets_at {
        Some(reset) if at >= reset => {
            let since = at - reset;
            let since = window_length(&state.limit.window).map_or(since, |len| since % len);
            p * since as f64
        }
        _ => last + p * (at - last_at) as f64,
    };
    Some(used.clamp(0.0, 1.0))
}

/// "5h", "7d", "7d opus".
pub fn window_label(window: &str) -> String {
    match window {
        "five_hour" => "5h".into(),
        "seven_day" => "7d".into(),
        w => match w.strip_prefix("seven_day_") {
            Some(scope) => format!("7d {}", scope.replace('_', " ")),
            None => w.replace('_', " "),
        },
    }
}

/// What a job is for, as admission sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobClass {
    /// New work: held near the limit.
    Work,
    /// Checking finished work (a review): runs until the window is full, so
    /// verification is never starved by the work it checks.
    Verify,
}

/// When the governor holds or downgrades work.
#[derive(Clone, Debug, PartialEq)]
pub struct GovernorPolicy {
    /// New work waits while a window is at least this full (0 to 1).
    pub hold_at: f64,
    /// Work on a pricier model than `downgrade_to` runs on it from here,
    /// or when a window is forecast to reach `hold_at` within `horizon`.
    pub downgrade_at: f64,
    pub downgrade_to: String,
    /// How far ahead a job's window use is forecast.
    pub horizon: Millis,
    /// New work waits while the user's inbox load is above this share
    /// (0 to 1); `1.0` or more never holds.
    pub load_hold_at: f64,
}

impl Default for GovernorPolicy {
    fn default() -> GovernorPolicy {
        GovernorPolicy {
            hold_at: 0.97,
            downgrade_at: 0.90,
            downgrade_to: "sonnet".into(),
            horizon: 30 * MINUTE,
            load_hold_at: crate::alarm::HOLD_AT,
        }
    }
}

/// Whether a queued job may start.
#[derive(Clone, Debug, PartialEq)]
pub enum Admission {
    Admit,
    /// Wait for `window` to reset (at `until`, when known).
    Defer {
        window: String,
        used: f64,
        until: Option<Millis>,
    },
    /// Start on the cheaper model `to`: `window` is at `used` and is
    /// forecast at `ahead` by the end of the policy's horizon.
    Downgrade {
        from: String,
        to: String,
        window: String,
        used: f64,
        ahead: f64,
    },
    /// Wait: the user is working the inbox as much as they can take, so
    /// no new work starts until the load is back under `at`.
    Hold {
        load: f64,
        at: f64,
    },
}

impl Admission {
    pub fn is_deferred(&self) -> bool {
        matches!(self, Admission::Defer { .. })
    }

    /// Held back by the supervisor's load, not by a usage window.
    pub fn is_held(&self) -> bool {
        matches!(self, Admission::Hold { .. })
    }

    /// "deferred: 5h at 94%, resumes 15:58"; `None` when simply admitted.
    pub fn reason(&self, offset_min: i32) -> Option<String> {
        let pct = |u: f64| (u * 100.0).round().clamp(0.0, 100.0) as u32;
        match self {
            Admission::Admit => None,
            Admission::Hold { load, .. } => Some(format!("held: you're at {}% load", pct(*load))),
            Admission::Defer {
                window,
                used,
                until,
            } => Some(format!(
                "deferred: {} at {}%, {}",
                window_label(window),
                pct(*used),
                until.map_or_else(
                    || "resumes after the reset".to_string(),
                    |at| format!("resumes {}", local_clock(at, offset_min))
                )
            )),
            Admission::Downgrade {
                from,
                to,
                window,
                used,
                ahead,
            } => {
                let mut why = format!(
                    "{} → {}: {} at {}%",
                    Family::of(from).as_str(),
                    to,
                    window_label(window),
                    pct(*used)
                );
                if pct(*ahead) > pct(*used) {
                    why.push_str(&format!(", heading for {}%", pct(*ahead)));
                }
                Some(why)
            }
        }
    }
}

/// Price rank of a model: haiku < sonnet < opus < fable. `None` when unknown.
fn tier(model: &str) -> Option<u8> {
    if model.to_ascii_lowercase().contains("fable") {
        return Some(3);
    }
    match Family::of(model) {
        Family::Haiku => Some(0),
        Family::Sonnet => Some(1),
        Family::Opus => Some(2),
        Family::Other => None,
    }
}

/// The share used of a window that hasn't reset; `None` once it has, or
/// when the reading carries none.
fn live_use(w: &LimitState, now: Millis) -> Option<f64> {
    if w.is_stale(now) {
        return None;
    }
    let used = w.limit.utilization.filter(|u| u.is_finite());
    if w.limit.status == "rejected" {
        return Some(used.unwrap_or(1.0).max(1.0));
    }
    used
}

/// Whether a job on `model` may start now, given the latest reading of
/// every window. A window whose reset has passed counts as empty, so
/// deferred work is admitted again once it resets.
pub fn admit(
    policy: &GovernorPolicy,
    windows: &[LimitState],
    model: &str,
    class: JobClass,
    now: Millis,
) -> Admission {
    let limit = match class {
        JobClass::Work => policy.hold_at,
        JobClass::Verify => 1.0,
    };
    let held = windows
        .iter()
        .filter_map(|w| live_use(w, now).map(|u| (w, u)))
        .filter(|(_, u)| *u >= limit)
        .max_by_key(|(w, _)| w.limit.resets_at.unwrap_or(Millis::MAX));
    if let Some((w, used)) = held {
        return Admission::Defer {
            window: w.limit.window.clone(),
            used,
            until: w.limit.resets_at,
        };
    }
    let cheaper = tier(model)
        .zip(tier(&policy.downgrade_to))
        .is_some_and(|(m, to)| m > to);
    if class == JobClass::Verify || !cheaper {
        return Admission::Admit;
    }
    let pressed = windows
        .iter()
        .filter_map(|w| {
            let used = live_use(w, now)?;
            let ahead = forecast(w, now + policy.horizon).unwrap_or(used).max(used);
            (used >= policy.downgrade_at || ahead >= policy.hold_at).then_some((w, used, ahead))
        })
        .max_by(|a, b| a.2.total_cmp(&b.2));
    match pressed {
        Some((w, used, ahead)) => Admission::Downgrade {
            from: model.to_string(),
            to: policy.downgrade_to.clone(),
            window: w.limit.window.clone(),
            used,
            ahead,
        },
        None => Admission::Admit,
    }
}

/// `admission`, unless the user is working the inbox beyond the policy's
/// load line: then new work waits. A usage window that already defers the
/// job says so first, and verification is never held (it clears the inbox).
pub fn hold_for_load(
    policy: &GovernorPolicy,
    admission: Admission,
    class: JobClass,
    load: Option<f64>,
) -> Admission {
    match (admission, load) {
        (Admission::Admit | Admission::Downgrade { .. }, Some(load))
            if class == JobClass::Work && load.is_finite() && load > policy.load_hold_at =>
        {
            Admission::Hold {
                load,
                at: policy.load_hold_at,
            }
        }
        (admission, _) => admission,
    }
}

/// Whether a job's error says a usage limit stopped it.
pub fn is_limit_error(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    ["usage limit", "rate limit", "rate_limit", "limit reached"]
        .iter()
        .any(|k| t.contains(k))
}

/// The run limits on battery: at most `battery_max` jobs at once.
pub fn power_limits(limits: Limits, on_battery: bool, battery_max: usize) -> Limits {
    Limits {
        max_concurrent: if on_battery {
            limits.max_concurrent.min(battery_max.max(1))
        } else {
            limits.max_concurrent
        },
        ..limits
    }
}

/// The line a job is told about its budget, e.g. "[budget] up to $2.40 for
/// this run · 5h 62%, resets 15:58 · 7d 91% · new work holds at 97%".
/// `None` when there is nothing to say.
pub fn budget_line(
    windows: &[LimitState],
    cap_usd: Option<f64>,
    hold_at: f64,
    now: Millis,
    offset_min: i32,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(cap) = cap_usd.filter(|c| c.is_finite() && *c > 0.0) {
        parts.push(format!("up to ${cap:.2} for this run"));
    }
    let mut live: Vec<(&LimitState, f64)> = windows
        .iter()
        .filter_map(|w| live_use(w, now).map(|u| (w, u)))
        .collect();
    live.sort_by_key(|(w, _)| window_length(&w.limit.window).unwrap_or(Millis::MAX));
    for (w, used) in &live {
        let mut part = format!(
            "{} {:.0}%",
            window_label(&w.limit.window),
            (used * 100.0).clamp(0.0, 100.0)
        );
        if let Some(reset) = w.limit.resets_at.filter(|r| r - now <= 24 * HOUR) {
            part.push_str(&format!(", resets {}", local_clock(reset, offset_min)));
        }
        parts.push(part);
    }
    if parts.is_empty() {
        return None;
    }
    if !live.is_empty() {
        parts.push(format!(
            "new work holds at {:.0}%",
            (hold_at * 100.0).clamp(0.0, 100.0)
        ));
    }
    Some(format!("[budget] {}", parts.join(" · ")))
}

#[cfg(test)]
mod tests;
