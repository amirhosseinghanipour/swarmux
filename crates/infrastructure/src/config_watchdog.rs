//! The `[watchdog]` section of `config.toml`.

use std::path::Path;

use serde::Deserialize;
use swarmux_application::WatchdogSettings;
use swarmux_domain::Rung;

use crate::config::{ConfigError, key_line};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawWatchdog {
    enabled: Option<bool>,
    rungs: Option<Vec<String>>,
    stall_secs: Option<i64>,
    stall_tool_secs: Option<i64>,
    stall_turns: Option<i64>,
    oscillation_repeats: Option<i64>,
    context_high: Option<f64>,
    compactions: Option<i64>,
    burn_usd_per_min: Option<f64>,
    nudge_cooldown_secs: Option<i64>,
    restart_cooldown_secs: Option<i64>,
    max_restarts: Option<i64>,
    escalate_to: Option<Vec<String>>,
}

pub(crate) fn parse_watchdog(
    raw: RawWatchdog,
    text: &str,
    path: &Path,
) -> Result<WatchdogSettings, ConfigError> {
    let error = |key: &str, message: String| ConfigError {
        path: path.to_path_buf(),
        line: key_line(text, key),
        message,
    };
    let count = |key: &str, v: i64, min: i64, max: i64| -> Result<i64, ConfigError> {
        if (min..=max).contains(&v) {
            Ok(v)
        } else {
            Err(error(key, format!("{key} must be between {min} and {max}")))
        }
    };
    let mut w = WatchdogSettings::default();
    if let Some(on) = raw.enabled {
        w.enabled = on;
    }
    if let Some(names) = raw.rungs {
        let mut rungs = Vec::new();
        for name in &names {
            let rung = Rung::parse(name).ok_or_else(|| {
                let all: Vec<&str> = Rung::ALL.iter().map(|r| r.as_str()).collect();
                error(
                    "rungs",
                    format!("unknown rung {name:?}: use any of {}", all.join(", ")),
                )
            })?;
            rungs.push(rung);
        }
        w.ladder.rungs = rungs;
    }
    if let Some(v) = raw.stall_secs {
        w.limits.stall_silent_ms = count("stall_secs", v, 30, 3_600)? * 1_000;
    }
    if let Some(v) = raw.stall_tool_secs {
        w.limits.stall_tool_ms = count("stall_tool_secs", v, 60, 24 * 3_600)? * 1_000;
    }
    if let Some(v) = raw.stall_turns {
        w.limits.stall_turns = count("stall_turns", v, 0, 1_000)? as u32;
    }
    if let Some(v) = raw.oscillation_repeats {
        w.limits.oscillation_repeats = count("oscillation_repeats", v, 0, 100)? as u32;
    }
    if let Some(v) = raw.context_high {
        if !(v.is_finite() && v > 0.0 && v <= 1.0) {
            return Err(error(
                "context_high",
                "context_high must be a share above 0 and at most 1, like 0.85".into(),
            ));
        }
        w.limits.context_high = v;
    }
    if let Some(v) = raw.compactions {
        w.limits.compactions_high = count("compactions", v, 0, 100)? as u32;
    }
    if let Some(v) = raw.burn_usd_per_min {
        if !(v.is_finite() && v >= 0.0) {
            return Err(error(
                "burn_usd_per_min",
                "burn_usd_per_min must be a dollar amount, 0 to turn it off".into(),
            ));
        }
        w.limits.burn_usd_per_min = v;
    }
    if let Some(v) = raw.nudge_cooldown_secs {
        w.ladder.nudge_cooldown_ms = count("nudge_cooldown_secs", v, 10, 3_600)? * 1_000;
    }
    if let Some(v) = raw.restart_cooldown_secs {
        w.ladder.restart_cooldown_ms = count("restart_cooldown_secs", v, 10, 3_600)? * 1_000;
    }
    if let Some(v) = raw.max_restarts {
        w.ladder.max_restarts = count("max_restarts", v, 0, 10)? as u32;
    }
    if let Some(models) = raw.escalate_to {
        if let Some(bad) = models
            .iter()
            .find(|m| m.trim().is_empty() || m.contains(char::is_whitespace))
        {
            return Err(error(
                "escalate_to",
                format!("escalate_to takes model names like \"opus\", not {bad:?}"),
            ));
        }
        w.escalate_to = models;
    }
    Ok(w)
}
