//! What Claude Code's `statusLine` command is given: the one source of the
//! subscription windows as the account sees them, of prompt-cache health and
//! of the pull request of a session's branch.

use super::limits::RateLimit;
use super::time::short_span;
use crate::{JobId, Millis};

/// Where a limit reading came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LimitSource {
    /// The `rate_limit_event` of a swarmux job's stream.
    #[default]
    RateLimitEvent,
    /// The status line command of a session.
    Statusline,
}

impl LimitSource {
    pub fn label(self) -> &'static str {
        match self {
            LimitSource::RateLimitEvent => "stream",
            LimitSource::Statusline => "statusline",
        }
    }
}

/// One subscription window as the status line reports it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowReading {
    /// Share of the window used, 0 to 1.
    pub used: f64,
    pub resets_at: Option<Millis>,
}

/// The prompt cache of the main conversation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PromptCache {
    /// The cache still holds the conversation.
    pub warm: Option<bool>,
    /// How long entries live: `5m` or `1h`.
    pub ttl: Option<String>,
    pub expires_at: Option<Millis>,
    /// Share of cacheable tokens served from the cache, 0 to 1.
    pub hit_ratio: Option<f64>,
    pub requests: Option<u64>,
    pub misses: Option<u64>,
}

impl PromptCache {
    /// Time until the cache goes cold (0 once passed).
    pub fn expires_in(&self, now: Millis) -> Option<Millis> {
        self.expires_at.map(|at| (at - now).max(0))
    }

    /// The next request pays full price for the conversation.
    pub fn is_cold(&self, now: Millis) -> bool {
        self.warm == Some(false) || self.expires_at.is_some_and(|at| at <= now)
    }
}

/// The pull request (or merge request) of the session's branch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PullRef {
    pub number: Option<u64>,
    pub url: Option<String>,
    /// `approved`, `pending`, `changes_requested`, `draft`.
    pub review_state: Option<String>,
    /// `pr` or `mr`.
    pub kind: Option<String>,
}

/// One status line payload, reduced to what swarmux uses.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Statusline {
    pub session_id: String,
    /// The swarmux job running the session, when it is one of ours.
    pub job: Option<JobId>,
    /// When the payload was received.
    pub at: Millis,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub cost_usd: Option<f64>,
    /// Share of the context window used, 0 to 1.
    pub context_used: Option<f64>,
    pub context_window: Option<u64>,
    pub five_hour: Option<WindowReading>,
    pub seven_day: Option<WindowReading>,
    pub cache: Option<PromptCache>,
    pub pr: Option<PullRef>,
}

impl Statusline {
    /// The subscription windows this payload carries.
    pub fn rate_limits(&self) -> Vec<RateLimit> {
        [("five_hour", self.five_hour), ("seven_day", self.seven_day)]
            .into_iter()
            .filter_map(|(window, reading)| {
                let r = reading?;
                Some(RateLimit {
                    window: window.into(),
                    status: if r.used >= 1.0 { "rejected" } else { "allowed" }.into(),
                    utilization: Some(r.used),
                    resets_at: r.resets_at,
                    surpassed_threshold: None,
                })
            })
            .collect()
    }

    /// The one line the status line command prints.
    pub fn compact(&self, now: Millis) -> String {
        let pct = |f: f64| format!("{:.0}%", (f * 100.0).clamp(0.0, 999.0));
        let mut parts: Vec<String> = Vec::new();
        if let Some(model) = &self.model {
            parts.push(model.clone());
        }
        if let Some(c) = self.context_used {
            parts.push(format!("ctx {}", pct(c)));
        }
        if let Some(w) = self.five_hour {
            parts.push(format!("5h {}", pct(w.used)));
        }
        if let Some(w) = self.seven_day {
            parts.push(format!("7d {}", pct(w.used)));
        }
        if let Some(cache) = &self.cache {
            let hit = cache.hit_ratio.map(pct);
            parts.push(if cache.is_cold(now) {
                match hit {
                    Some(h) => format!("cache cold ({h})"),
                    None => "cache cold".into(),
                }
            } else {
                match (hit, cache.expires_in(now)) {
                    (Some(h), Some(left)) => format!("cache {h} {}", short_span(left)),
                    (Some(h), None) => format!("cache {h}"),
                    (None, Some(left)) => format!("cache {}", short_span(left)),
                    (None, None) => "cache warm".into(),
                }
            });
        }
        if let Some(pr) = &self.pr {
            let label = pr.kind.as_deref().unwrap_or("PR").to_uppercase();
            let mut text = pr.number.map_or(label.clone(), |n| format!("{label} #{n}"));
            if let Some(state) = &pr.review_state {
                text.push(' ');
                text.push_str(&state.replace('_', " "));
            }
            parts.push(text);
        }
        if let Some(cost) = self.cost_usd {
            parts.push(format!("${cost:.2}"));
        }
        if parts.is_empty() {
            "swarmux".into()
        } else {
            parts.join(" · ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::{HOUR, MINUTE};

    fn full() -> Statusline {
        Statusline {
            session_id: "s".into(),
            model: Some("Haiku".into()),
            context_used: Some(0.12),
            five_hour: Some(WindowReading {
                used: 0.41,
                resets_at: Some(HOUR),
            }),
            seven_day: Some(WindowReading {
                used: 0.9,
                resets_at: None,
            }),
            cache: Some(PromptCache {
                warm: Some(true),
                expires_at: Some(5 * MINUTE),
                hit_ratio: Some(0.94),
                ..PromptCache::default()
            }),
            pr: Some(PullRef {
                number: Some(12),
                review_state: Some("changes_requested".into()),
                kind: Some("pr".into()),
                ..PullRef::default()
            }),
            cost_usd: Some(0.5),
            ..Statusline::default()
        }
    }

    #[test]
    fn the_compact_line_names_what_is_known() {
        assert_eq!(
            full().compact(2 * MINUTE),
            "Haiku · ctx 12% · 5h 41% · 7d 90% · cache 94% 3m · PR #12 changes requested · $0.50"
        );
        assert_eq!(Statusline::default().compact(0), "swarmux");
    }

    #[test]
    fn a_cold_cache_says_so() {
        let mut s = full();
        if let Some(c) = s.cache.as_mut() {
            c.warm = Some(false);
        }
        assert!(s.compact(0).contains("cache cold (94%)"));
        let mut s = full();
        assert!(s.cache.as_ref().is_some_and(|c| c.is_cold(10 * MINUTE)));
        s.cache = None;
        assert!(!s.compact(0).contains("cache"));
    }

    #[test]
    fn windows_become_rate_limits() {
        let limits = full().rate_limits();
        assert_eq!(limits.len(), 2);
        assert_eq!(limits[0].window, "five_hour");
        assert_eq!(limits[0].utilization, Some(0.41));
        assert_eq!(limits[0].status, "allowed");
        let mut s = full();
        s.seven_day = Some(WindowReading {
            used: 1.0,
            resets_at: None,
        });
        assert_eq!(s.rate_limits()[1].status, "rejected");
    }
}
