//! Reads the JSON Claude Code gives a `statusLine` command.
//!
//! Field names and units have drifted across Claude Code versions (the
//! windows are `five_hour` and `seven_day`, older readers saw `session` and
//! `weekly`; times are epoch seconds or ISO text; shares are 0 to 1 or 0 to
//! 100), so every field is optional and read in whichever form it comes.

use serde_json::Value;
use swarmux_domain::telemetry::{PromptCache, PullRef, Statusline, WindowReading};
use swarmux_domain::{JobId, Millis, parse_iso_millis};

/// Epoch values below this are seconds, not milliseconds.
const SECONDS_BELOW: f64 = 1.0e11;

/// Reads one payload. `None` when it isn't a JSON object with a session id.
/// `at` is when it arrived; `job` is the swarmux job running the session.
pub fn parse_statusline(input: &str, job: Option<JobId>, at: Millis) -> Option<Statusline> {
    let v: Value = serde_json::from_str(input).ok()?;
    let session_id = text(v.get("session_id")?)?;
    let windows = v.get("rate_limits");
    let window = |names: &[&str]| -> Option<WindowReading> {
        let w = names.iter().find_map(|n| windows?.get(*n))?;
        let used = share(
            w.get("used_percentage").or_else(|| w.get("utilization")),
            true,
        )?;
        Some(WindowReading {
            used,
            resets_at: w.get("resets_at").and_then(time),
        })
    };
    let ctx = v.get("context_window");
    Some(Statusline {
        session_id,
        job,
        at,
        model: v.get("model").and_then(model_name),
        cwd: v
            .pointer("/workspace/current_dir")
            .or_else(|| v.get("cwd"))
            .and_then(text),
        cost_usd: v.pointer("/cost/total_cost_usd").and_then(Value::as_f64),
        context_used: ctx.and_then(|c| share(c.get("used_percentage"), true)),
        context_window: ctx
            .and_then(|c| c.get("context_window_size"))
            .and_then(Value::as_u64),
        five_hour: window(&["five_hour", "session"]),
        seven_day: window(&["seven_day", "weekly", "weekly_all"]),
        cache: v.get("prompt_cache").and_then(cache),
        pr: v.get("pr").and_then(pull),
    })
}

fn text(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn model_name(v: &Value) -> Option<String> {
    match v {
        Value::String(_) => text(v),
        _ => v.get("display_name").or_else(|| v.get("id")).and_then(text),
    }
}

/// A share as 0 to 1. `percent` says plain numbers are percentages; a
/// number above 1 is read as a percentage either way.
fn share(v: Option<&Value>, percent: bool) -> Option<f64> {
    let n = v?.as_f64().filter(|n| n.is_finite() && *n >= 0.0)?;
    Some(if percent || n > 1.0 { n / 100.0 } else { n })
}

/// Epoch seconds, epoch milliseconds or an ISO timestamp.
fn time(v: &Value) -> Option<Millis> {
    match v {
        Value::Number(n) => {
            let n = n.as_f64().filter(|n| n.is_finite() && *n > 0.0)?;
            Some(if n < SECONDS_BELOW { n * 1000.0 } else { n } as Millis)
        }
        Value::String(s) => parse_iso_millis(s).or_else(|| {
            s.trim()
                .parse::<f64>()
                .ok()
                .and_then(|n| time(&Value::from(n)))
        }),
        _ => None,
    }
}

fn cache(v: &Value) -> Option<PromptCache> {
    v.as_object()?;
    Some(PromptCache {
        warm: v.get("warm").and_then(Value::as_bool),
        ttl: v.get("ttl").and_then(|t| match t {
            Value::Number(n) => n.as_u64().map(|s| format!("{}m", s / 60)),
            other => text(other),
        }),
        expires_at: v.get("expires_at").and_then(time),
        hit_ratio: share(v.get("hit_ratio"), false),
        requests: v.get("requests").and_then(Value::as_u64),
        misses: v.get("misses").and_then(Value::as_u64),
    })
}

fn pull(v: &Value) -> Option<PullRef> {
    v.as_object()?;
    let pr = PullRef {
        number: v.get("number").and_then(Value::as_u64),
        url: v.get("url").and_then(text),
        review_state: v.get("review_state").and_then(text),
        kind: v.get("kind").and_then(text),
    };
    (pr != PullRef::default()).then_some(pr)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: Millis = 1_793_613_600_000;

    const FULL: &str = r#"{
      "session_id": "abc",
      "model": {"id": "claude-haiku-5-5", "display_name": "Haiku 5.5"},
      "workspace": {"current_dir": "C:\\w", "repo": {"host": "github.com", "owner": "o", "name": "n"}},
      "cost": {"total_cost_usd": 0.0123, "total_duration_ms": 5000},
      "context_window": {"used_percentage": 12.5, "context_window_size": 1000000},
      "rate_limits": {
        "five_hour": {"used_percentage": 41.0, "resets_at": 1793620000},
        "seven_day": {"used_percentage": 90, "resets_at": "2026-11-08T10:00:00Z"}
      },
      "prompt_cache": {"warm": true, "ttl": "1h", "expires_at": 1793617200,
                       "requests": 4, "misses": 1, "hit_ratio": 0.75},
      "pr": {"number": 9, "url": "https://github.com/o/n/pull/9", "review_state": "approved", "kind": "pr"}
    }"#;

    #[test]
    fn a_full_payload() {
        let s = parse_statusline(FULL, Some(JobId(4)), NOW).unwrap();
        assert_eq!(s.session_id, "abc");
        assert_eq!(s.job, Some(JobId(4)));
        assert_eq!(s.model.as_deref(), Some("Haiku 5.5"));
        assert_eq!(s.cwd.as_deref(), Some("C:\\w"));
        assert_eq!(s.cost_usd, Some(0.0123));
        assert_eq!(s.context_used, Some(0.125));
        assert_eq!(s.context_window, Some(1_000_000));
        let five = s.five_hour.unwrap();
        assert!((five.used - 0.41).abs() < 1e-9);
        assert_eq!(five.resets_at, Some(1_793_620_000_000));
        let seven = s.seven_day.unwrap();
        assert!((seven.used - 0.9).abs() < 1e-9);
        assert_eq!(seven.resets_at, parse_iso_millis("2026-11-08T10:00:00Z"));
        let cache = s.cache.unwrap();
        assert_eq!(cache.warm, Some(true));
        assert_eq!(cache.ttl.as_deref(), Some("1h"));
        assert_eq!(cache.expires_at, Some(1_793_617_200_000));
        assert_eq!(cache.hit_ratio, Some(0.75));
        assert_eq!((cache.requests, cache.misses), (Some(4), Some(1)));
        let pr = s.pr.unwrap();
        assert_eq!(pr.number, Some(9));
        assert_eq!(pr.review_state.as_deref(), Some("approved"));
    }

    #[test]
    fn older_window_names_and_missing_parts() {
        let s = parse_statusline(
            r#"{"session_id":"x","model":"sonnet","rate_limits":{"session":{"used_percentage":5},"weekly":{"used_percentage":50,"resets_at":1793620000000}}}"#,
            None,
            NOW,
        )
        .unwrap();
        assert_eq!(s.model.as_deref(), Some("sonnet"));
        assert!((s.five_hour.unwrap().used - 0.05).abs() < 1e-9);
        let seven = s.seven_day.unwrap();
        assert_eq!(
            seven.resets_at,
            Some(1_793_620_000_000),
            "milliseconds stay"
        );
        assert!(s.cache.is_none() && s.pr.is_none() && s.context_used.is_none());
    }

    #[test]
    fn a_hit_ratio_in_percent_is_scaled() {
        let s = parse_statusline(
            r#"{"session_id":"x","prompt_cache":{"hit_ratio":87.5}}"#,
            None,
            NOW,
        )
        .unwrap();
        assert_eq!(s.cache.unwrap().hit_ratio, Some(0.875));
    }

    #[test]
    fn junk_is_none() {
        assert!(parse_statusline("garbage", None, NOW).is_none());
        assert!(parse_statusline("{}", None, NOW).is_none());
        assert!(parse_statusline(r#"{"session_id":""}"#, None, NOW).is_none());
        let s = parse_statusline(
            r#"{"session_id":"x","rate_limits":{"five_hour":{"used_percentage":"lots"}},"pr":{},"prompt_cache":7}"#,
            None,
            NOW,
        )
        .unwrap();
        assert!(s.five_hour.is_none() && s.pr.is_none() && s.cache.is_none());
    }
}
