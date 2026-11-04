//! The watchdog's detectors: signs that a running agent is stuck, read from
//! its events and a few gauges. They extend the loop breaker: the same
//! failure in a row is still the loop, and the rest looks wider.
//!
//! Pure: time is passed in, nothing is read or written.

use std::collections::{HashMap, HashSet, VecDeque};

use super::loop_breaker::{LoopBreaker, error_signature};
use crate::telemetry::{PriceTable, TokenUsage};
use crate::{AgentEvent, Millis};

/// Tools whose success changes files: progress.
const EDIT_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];
/// Tools that run something for as long as it takes.
const LONG_TOOLS: [&str; 4] = ["Bash", "PowerShell", "Task", "Agent"];

/// What kind of trouble a detector saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AlarmKind {
    /// The same call failed the same way, again and again.
    Loop,
    /// A call keeps failing the same way between other steps: fix, test,
    /// same error, fix, test, same error.
    Oscillation,
    /// No events for too long, or many calls that only repeat earlier ones.
    Stall,
    /// The context window is nearly full, or compacted more than once.
    Context,
    /// Spending much faster than usual.
    Burn,
}

impl AlarmKind {
    pub const ALL: [AlarmKind; 5] = [
        AlarmKind::Loop,
        AlarmKind::Oscillation,
        AlarmKind::Stall,
        AlarmKind::Context,
        AlarmKind::Burn,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            AlarmKind::Loop => "loop",
            AlarmKind::Oscillation => "oscillation",
            AlarmKind::Stall => "stall",
            AlarmKind::Context => "context",
            AlarmKind::Burn => "burn",
        }
    }

    /// The glyph the session card shows.
    pub fn glyph(self) -> &'static str {
        match self {
            AlarmKind::Loop | AlarmKind::Oscillation => "⟲",
            AlarmKind::Stall => "◐",
            AlarmKind::Context => "◔",
            AlarmKind::Burn => "⚡",
        }
    }
}

/// One detector firing.
#[derive(Clone, Debug, PartialEq)]
pub struct WatchAlarm {
    pub kind: AlarmKind,
    /// One line saying what was seen.
    pub detail: String,
    pub at: Millis,
    /// The agent is still making tool calls, so a message on its next call
    /// reaches it. A silent agent can't be reached that way.
    pub reachable: bool,
    /// The call that keeps failing, as (tool, summary), when there is one.
    pub call: Option<(String, String)>,
    /// Its error's signature.
    pub error: Option<String>,
}

/// When the detectors fire.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WatchLimits {
    /// Identical failures in a row that count as a loop (0 = off).
    pub loop_repeats: u32,
    /// Times one call fails the same way between other steps (0 = off).
    pub oscillation_repeats: u32,
    /// Silence, with no tool running, that counts as a stall.
    pub stall_silent_ms: Millis,
    /// How long one command or subagent may run before it counts as a stall.
    pub stall_tool_ms: Millis,
    /// Calls in a row that only repeat earlier calls (0 = off).
    pub stall_turns: u32,
    /// Share of the context window that counts as pressure.
    pub context_high: f64,
    /// Compactions of one job that count as pressure (0 = off).
    pub compactions_high: u32,
    /// Estimated dollars per minute that count as burning (0 = off).
    pub burn_usd_per_min: f64,
    /// The window the spending rate is measured over.
    pub burn_window_ms: Millis,
}

impl Default for WatchLimits {
    fn default() -> WatchLimits {
        WatchLimits {
            loop_repeats: 2,
            oscillation_repeats: 3,
            stall_silent_ms: 120_000,
            stall_tool_ms: 600_000,
            stall_turns: 15,
            context_high: 0.85,
            compactions_high: 2,
            burn_usd_per_min: 1.0,
            burn_window_ms: 300_000,
        }
    }
}

/// What the detectors read besides the events.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Gauges {
    /// Lead context in use, `0.0..` of the window, when known.
    pub context: Option<f64>,
    /// Times the job's context was compacted.
    pub compactions: u32,
    /// The agent waits for the user (an approval or a question): never a stall.
    pub waiting_on_user: bool,
}

type CallKey = (String, String);

/// How a call keeps failing.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Failing {
    sig: String,
    /// Times in a row it failed this way, whatever came in between.
    count: u32,
    /// The latest such failure.
    at: Millis,
}

/// What a tool result carries when the watchdog's own message stopped the call.
pub const NUDGE_MARK: &str = "[swarmux watchdog] ";

/// Watches one job.
#[derive(Clone, Debug)]
pub struct Detector {
    limits: WatchLimits,
    breaker: LoopBreaker,
    model: String,
    prices: PriceTable,
    /// tool_use id → the call.
    calls: HashMap<String, CallKey>,
    /// Long-running calls waiting for their result, by tool_use id.
    open: HashMap<String, (CallKey, Millis)>,
    /// How each failing call keeps failing.
    failures: HashMap<CallKey, Failing>,
    seen: HashSet<CallKey>,
    repeats: u32,
    last_event_at: Millis,
    /// The last file change, todo update, or call never made before.
    progress_at: Millis,
    /// Estimated spend, by when it happened.
    spend: VecDeque<(Millis, f64)>,
    started_at: Millis,
    /// Tick alarms raised and not yet cleared, so each fires once.
    raised: HashSet<AlarmKind>,
}

impl Detector {
    pub fn new(limits: WatchLimits, model: &str, now: Millis) -> Detector {
        Detector {
            limits,
            breaker: LoopBreaker::new(limits.loop_repeats),
            model: model.to_string(),
            prices: PriceTable::builtin(),
            calls: HashMap::new(),
            open: HashMap::new(),
            failures: HashMap::new(),
            seen: HashSet::new(),
            repeats: 0,
            last_event_at: now,
            progress_at: now,
            spend: VecDeque::new(),
            started_at: now,
            raised: HashSet::new(),
        }
    }

    /// When the agent last did something new: changed a file, updated its
    /// todo list, or made a call it never made before.
    pub fn progress_at(&self) -> Millis {
        self.progress_at
    }

    /// When the last event arrived.
    pub fn last_event_at(&self) -> Millis {
        self.last_event_at
    }

    /// Feeds one event. Returns an alarm when it completes a loop, an
    /// oscillation or a run of repeated calls.
    pub fn observe(&mut self, ev: &AgentEvent, now: Millis) -> Option<WatchAlarm> {
        self.last_event_at = now;
        self.raised.remove(&AlarmKind::Stall);
        let trip = self.breaker.observe(ev);
        match ev {
            AgentEvent::ToolUse {
                id, name, summary, ..
            } => {
                let key = (name.clone(), summary.clone());
                if LONG_TOOLS.contains(&name.as_str()) {
                    self.open.insert(id.clone(), (key.clone(), now));
                }
                self.calls.insert(id.clone(), key.clone());
                if self.seen.insert(key) {
                    self.progress(now);
                    return None;
                }
                self.repeats += 1;
                let turns = self.limits.stall_turns;
                (turns > 0 && self.repeats == turns).then(|| WatchAlarm {
                    kind: AlarmKind::Stall,
                    detail: format!("{turns} calls in a row only repeat earlier ones; nothing new"),
                    at: now,
                    reachable: true,
                    call: None,
                    error: None,
                })
            }
            AgentEvent::ToolResult {
                tool_use_id,
                is_error,
                preview,
                ..
            } => {
                self.open.remove(tool_use_id);
                let key = self.calls.remove(tool_use_id)?;
                if !*is_error {
                    self.failures.remove(&key);
                    if EDIT_TOOLS.contains(&key.0.as_str()) {
                        self.progress(now);
                    }
                    return None;
                }
                // The watchdog's own message is not the agent failing.
                if preview.contains(NUDGE_MARK) {
                    return None;
                }
                let sig = error_signature(preview);
                let entry = self.failures.entry(key.clone()).or_insert(Failing {
                    sig: sig.clone(),
                    count: 0,
                    at: now,
                });
                if entry.sig == sig {
                    entry.count += 1;
                } else {
                    entry.sig = sig.clone();
                    entry.count = 1;
                }
                entry.at = now;
                let n = entry.count;
                if let Some(trip) = trip {
                    return Some(WatchAlarm {
                        kind: AlarmKind::Loop,
                        detail: format!(
                            "{} failed the same way {} times: {}",
                            show_call(&key),
                            trip.repeats,
                            trip.error
                        ),
                        at: now,
                        reachable: true,
                        call: Some(key),
                        error: Some(sig),
                    });
                }
                let limit = self.limits.oscillation_repeats;
                (limit > 0 && n >= limit).then(|| WatchAlarm {
                    kind: AlarmKind::Oscillation,
                    detail: format!(
                        "{} failed the same way {n} times between other steps: {sig}",
                        show_call(&key)
                    ),
                    at: now,
                    reachable: true,
                    call: Some(key),
                    error: Some(sig),
                })
            }
            AgentEvent::Todos { .. } => {
                self.progress(now);
                None
            }
            AgentEvent::Usage {
                input,
                output,
                cache_read,
                cache_creation,
                ..
            } => {
                let usage = TokenUsage {
                    input: *input,
                    output: *output,
                    cache_read: *cache_read,
                    cache_write_5m: *cache_creation,
                    cache_write_1h: 0,
                };
                if let Some(cost) = self.prices.cost(&self.model, &usage) {
                    self.spend.push_back((now, cost));
                }
                None
            }
            _ => None,
        }
    }

    fn progress(&mut self, now: Millis) {
        self.progress_at = now;
        self.repeats = 0;
    }

    /// Checks what needs no event: silence, context and spending. Each
    /// alarm fires once until its condition clears.
    pub fn tick(&mut self, now: Millis, gauges: &Gauges) -> Vec<WatchAlarm> {
        let mut out = Vec::new();
        let horizon = now - self.limits.burn_window_ms;
        while self.spend.front().is_some_and(|(at, _)| *at < horizon) {
            self.spend.pop_front();
        }
        let checks = [
            (AlarmKind::Stall, self.stall(now, gauges)),
            (AlarmKind::Context, self.pressure(gauges)),
            (AlarmKind::Burn, self.burn(now)),
        ];
        for (kind, found) in checks {
            match found {
                Some((detail, reachable)) => {
                    if self.raised.insert(kind) {
                        out.push(WatchAlarm {
                            kind,
                            detail,
                            at: now,
                            reachable,
                            call: None,
                            error: None,
                        });
                    }
                }
                None => {
                    self.raised.remove(&kind);
                }
            }
        }
        out
    }

    fn stall(&self, now: Millis, g: &Gauges) -> Option<(String, bool)> {
        if g.waiting_on_user {
            return None;
        }
        if let Some((key, since)) = self.open.values().min_by_key(|(_, at)| *at) {
            let ran = now - since;
            return (ran >= self.limits.stall_tool_ms).then(|| {
                (
                    format!("{} has run for {} min", show_call(key), ran / 60_000),
                    false,
                )
            });
        }
        let quiet = now - self.last_event_at;
        (quiet >= self.limits.stall_silent_ms)
            .then(|| (format!("no events for {} s", quiet / 1_000), false))
    }

    fn pressure(&self, g: &Gauges) -> Option<(String, bool)> {
        if let Some(c) = g.context.filter(|c| *c >= self.limits.context_high) {
            return Some((format!("context {:.0}% full", (c * 100.0).min(999.0)), true));
        }
        let high = self.limits.compactions_high;
        (high > 0 && g.compactions >= high)
            .then(|| (format!("context compacted {} times", g.compactions), true))
    }

    fn burn(&self, now: Millis) -> Option<(String, bool)> {
        let limit = self.limits.burn_usd_per_min;
        let window = self.limits.burn_window_ms;
        // Too early to tell a rate from the first big prompt.
        if limit <= 0.0 || window <= 0 || now - self.started_at < window / 2 {
            return None;
        }
        let spent: f64 = self
            .spend
            .iter()
            .filter(|(at, _)| *at >= now - window)
            .map(|(_, c)| c)
            .sum();
        let rate = spent / (window as f64 / 60_000.0);
        (rate >= limit).then(|| {
            (
                format!("spending about ${rate:.2}/min (limit ${limit:.2}/min)"),
                true,
            )
        })
    }

    /// Whether the trouble `alarm` reported still holds, judged from what
    /// happened since `since` (the alarm, or the last step taken against it).
    pub fn still(&self, alarm: &WatchAlarm, since: Millis, now: Millis, gauges: &Gauges) -> bool {
        match alarm.kind {
            // Still failing that way since, or nothing new since.
            AlarmKind::Loop | AlarmKind::Oscillation => match (&alarm.call, &alarm.error) {
                (Some(call), Some(sig)) => self
                    .failures
                    .get(call)
                    .is_some_and(|f| f.sig == *sig && (f.at > since || self.progress_at <= since)),
                _ => self.progress_at <= since,
            },
            AlarmKind::Stall if alarm.reachable => self.progress_at <= since,
            AlarmKind::Stall => self.last_event_at <= since || self.stall(now, gauges).is_some(),
            AlarmKind::Context => self.pressure(gauges).is_some(),
            AlarmKind::Burn => self.burn(now).is_some(),
        }
    }
}

/// `Bash cargo test`, cut to a readable length.
pub fn show_call((tool, summary): &(String, String)) -> String {
    let summary: String = summary.chars().take(80).collect();
    if summary.is_empty() || summary == *tool {
        tool.clone()
    } else {
        format!("{tool} `{summary}`")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Actor;

    const S: Millis = 1_000;

    fn call(id: &str, tool: &str, summary: &str) -> AgentEvent {
        AgentEvent::ToolUse {
            id: id.into(),
            name: tool.into(),
            summary: summary.into(),
            actor: Actor::Lead,
        }
    }

    fn result(id: &str, is_error: bool, preview: &str) -> AgentEvent {
        AgentEvent::ToolResult {
            tool_use_id: id.into(),
            is_error,
            preview: preview.into(),
            actor: Actor::Lead,
        }
    }

    fn det() -> Detector {
        Detector::new(WatchLimits::default(), "sonnet", 0)
    }

    #[test]
    fn same_failure_twice_is_a_loop() {
        let mut d = det();
        d.observe(&call("1", "Bash", "cargo test"), S);
        assert_eq!(d.observe(&result("1", true, "error 1"), S), None);
        d.observe(&call("2", "Bash", "cargo test"), 2 * S);
        let a = d.observe(&result("2", true, "error 2"), 2 * S).unwrap();
        assert_eq!(a.kind, AlarmKind::Loop);
        assert_eq!(a.call, Some(("Bash".into(), "cargo test".into())));
        assert!(
            a.detail.contains("failed the same way 2 times"),
            "{}",
            a.detail
        );
    }

    #[test]
    fn same_failure_between_edits_is_an_oscillation() {
        let mut d = det();
        let mut alarm = None;
        for i in 0..3 {
            d.observe(&call(&format!("e{i}"), "Edit", "src/a.rs"), S);
            d.observe(&result(&format!("e{i}"), false, "ok"), S);
            d.observe(&call(&format!("t{i}"), "Bash", "cargo test"), S);
            alarm = d.observe(&result(&format!("t{i}"), true, "assertion failed: x"), S);
        }
        let a = alarm.unwrap();
        assert_eq!(a.kind, AlarmKind::Oscillation);
        assert!(a.detail.contains("3 times between other steps"));
    }

    #[test]
    fn different_errors_are_progress_not_oscillation() {
        let mut d = det();
        for (i, err) in ["e1 here", "e2 there", "e3 elsewhere", "e4 more"]
            .iter()
            .enumerate()
        {
            d.observe(&call(&format!("t{i}"), "Bash", "cargo test"), S);
            assert_eq!(d.observe(&result(&format!("t{i}"), true, err), S), None);
            d.observe(&call(&format!("e{i}"), "Edit", "src/a.rs"), S);
            d.observe(&result(&format!("e{i}"), false, "ok"), S);
        }
    }

    #[test]
    fn repeated_calls_without_progress_stall() {
        let mut d = det();
        d.observe(&call("a", "Read", "a.rs"), S);
        d.observe(&call("b", "Read", "b.rs"), S);
        let mut fired = Vec::new();
        for i in 0..20u32 {
            let f = if i.is_multiple_of(2) { "a.rs" } else { "b.rs" };
            fired.extend(d.observe(&call(&format!("r{i}"), "Read", f), S));
        }
        assert_eq!(fired.len(), 1, "fires once");
        assert_eq!(fired[0].kind, AlarmKind::Stall);
        assert!(fired[0].reachable);
    }

    #[test]
    fn new_calls_are_progress() {
        let mut d = det();
        for i in 0..40 {
            assert_eq!(
                d.observe(&call(&format!("r{i}"), "Read", &format!("f{i}")), S),
                None
            );
        }
    }

    #[test]
    fn silence_stalls_after_the_limit_and_once() {
        let mut d = det();
        d.observe(&call("1", "Read", "a"), 0);
        d.observe(&result("1", false, "x"), 0);
        let g = Gauges::default();
        assert!(d.tick(119 * S, &g).is_empty());
        let a = d.tick(120 * S, &g);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].kind, AlarmKind::Stall);
        assert!(!a[0].reachable);
        assert!(d.tick(130 * S, &g).is_empty(), "once");
        assert!(d.still(&a[0], 120 * S, 130 * S, &g));
        d.observe(&call("2", "Read", "b"), 131 * S);
        assert!(!d.still(&a[0], 120 * S, 131 * S, &g));
    }

    #[test]
    fn a_long_command_is_not_a_silent_stall() {
        let mut d = det();
        d.observe(&call("1", "Bash", "cargo build --release"), 0);
        let g = Gauges::default();
        assert!(d.tick(9 * 60 * S, &g).is_empty());
        let a = d.tick(10 * 60 * S, &g);
        assert_eq!(a.len(), 1);
        assert!(
            a[0].detail.contains("has run for 10 min"),
            "{}",
            a[0].detail
        );
    }

    #[test]
    fn waiting_on_the_user_is_never_a_stall() {
        let mut d = det();
        let g = Gauges {
            waiting_on_user: true,
            ..Gauges::default()
        };
        assert!(d.tick(60 * 60 * S, &g).is_empty());
    }

    #[test]
    fn context_pressure_from_fill_or_compactions() {
        let mut d = det();
        let full = Gauges {
            context: Some(0.9),
            ..Gauges::default()
        };
        let a = d.tick(S, &full);
        assert_eq!(a[0].kind, AlarmKind::Context);
        assert_eq!(a[0].detail, "context 90% full");
        let mut d = det();
        let squeezed = Gauges {
            compactions: 2,
            ..Gauges::default()
        };
        assert_eq!(d.tick(S, &squeezed)[0].detail, "context compacted 2 times");
        assert!(d.tick(S, &Gauges::default()).is_empty());
    }

    #[test]
    fn burning_is_measured_over_the_window() {
        let mut d = det();
        let g = Gauges::default();
        // Sonnet output at $10/MTok: 150k output tokens a minute is $1.50/min.
        for m in 0..6 {
            let usage = AgentEvent::Usage {
                input: 0,
                output: 150_000,
                cache_read: 0,
                cache_creation: 0,
                actor: Actor::Lead,
            };
            d.observe(&usage, m * 60 * S);
        }
        let a = d.tick(5 * 60 * S, &g);
        assert_eq!(a.len(), 1, "{a:?}");
        assert_eq!(a[0].kind, AlarmKind::Burn);
        // Quiet for a window: the rate falls and the alarm clears.
        assert!(!d.still(&a[0], 0, 11 * 60 * S, &g));
    }

    #[test]
    fn modest_spending_is_fine() {
        let mut d = det();
        for m in 0..10 {
            let usage = AgentEvent::Usage {
                input: 2_000,
                output: 3_000,
                cache_read: 80_000,
                cache_creation: 1_000,
                actor: Actor::Lead,
            };
            d.observe(&usage, m * 30 * S);
        }
        assert!(d.tick(5 * 60 * S, &Gauges::default()).is_empty());
    }

    #[test]
    fn show_call_is_short() {
        assert_eq!(show_call(&("Bash".into(), "ls".into())), "Bash `ls`");
        assert_eq!(show_call(&("Glob".into(), "Glob".into())), "Glob");
    }
}
