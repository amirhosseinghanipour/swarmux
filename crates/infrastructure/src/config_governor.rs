//! The `[governor]` section of `config.toml`.

use std::path::Path;

use serde::Deserialize;
use swarmux_application::GovernorSettings;

use crate::config::{ConfigError, key_line};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawGovernor {
    enabled: Option<bool>,
    hold_at: Option<f64>,
    downgrade_at: Option<f64>,
    downgrade_to: Option<String>,
    horizon_minutes: Option<i64>,
    battery_concurrency: Option<i64>,
    max_job_usd: Option<f64>,
    budget_line: Option<bool>,
    load_hold_at: Option<f64>,
}

pub(crate) fn parse_governor(
    raw: RawGovernor,
    text: &str,
    path: &Path,
) -> Result<GovernorSettings, ConfigError> {
    let error = |key: &str, message: String| ConfigError {
        path: path.to_path_buf(),
        line: key_line(text, key),
        message,
    };
    let share = |key: &str, v: f64| -> Result<f64, ConfigError> {
        if v.is_finite() && v > 0.0 && v <= 1.0 {
            Ok(v)
        } else {
            Err(error(
                key,
                format!("{key} must be a share above 0 and at most 1, like 0.97"),
            ))
        }
    };
    let mut g = GovernorSettings::default();
    if let Some(on) = raw.enabled {
        g.enabled = on;
    }
    if let Some(v) = raw.hold_at {
        g.hold_at = share("hold_at", v)?;
    }
    if let Some(v) = raw.downgrade_at {
        g.downgrade_at = share("downgrade_at", v)?;
    }
    if let Some(model) = raw.downgrade_to {
        let model = model.trim();
        if model.is_empty() || model.contains(char::is_whitespace) {
            return Err(error(
                "downgrade_to",
                "downgrade_to must be one word, like \"sonnet\" or \"haiku\"".into(),
            ));
        }
        g.downgrade_to = model.to_string();
    }
    if let Some(m) = raw.horizon_minutes {
        g.horizon_minutes = u32::try_from(m)
            .ok()
            .filter(|m| (1..=24 * 60).contains(m))
            .ok_or_else(|| {
                error(
                    "horizon_minutes",
                    "horizon_minutes must be between 1 and 1440".into(),
                )
            })?;
    }
    if let Some(n) = raw.battery_concurrency {
        g.battery_concurrency = usize::try_from(n).ok().filter(|n| *n >= 1).ok_or_else(|| {
            error(
                "battery_concurrency",
                "battery_concurrency must be at least 1".into(),
            )
        })?;
    }
    if let Some(usd) = raw.max_job_usd {
        if !(usd.is_finite() && usd > 0.0) {
            return Err(error(
                "max_job_usd",
                "max_job_usd must be a dollar amount above 0".into(),
            ));
        }
        g.max_job_usd = Some(usd);
    }
    if let Some(on) = raw.budget_line {
        g.budget_line = on;
    }
    if let Some(v) = raw.load_hold_at {
        g.load_hold_at = share("load_hold_at", v)?;
    }
    Ok(g)
}
