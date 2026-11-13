//! The `[missions]` section of `config.toml`: plan-first missions.

use std::path::Path;

use serde::Deserialize;
use swarmux_application::{LandingPolicy, PlanSettings};

use crate::config::{ConfigError, key_line};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawMissions {
    plan_first: Option<bool>,
    plan_model: Option<String>,
    drift_threshold: Option<f64>,
    parallel: Option<bool>,
    landing: Option<String>,
}

pub(crate) fn parse_missions(
    raw: RawMissions,
    text: &str,
    path: &Path,
) -> Result<PlanSettings, ConfigError> {
    let error = |key: &str, message: &str| ConfigError {
        path: path.to_path_buf(),
        line: key_line(text, key),
        message: message.to_string(),
    };
    let mut settings = PlanSettings::default();
    if let Some(on) = raw.plan_first {
        settings.plan_first = on;
    }
    if let Some(on) = raw.parallel {
        settings.parallel = on;
    }
    if let Some(word) = raw.landing {
        settings.landing = LandingPolicy::parse(word.trim()).ok_or_else(|| {
            error(
                "landing",
                &format!(
                    "landing must be \"auto\" (land when the gate passes) or \"manual\" (wait for you to land), not \"{word}\""
                ),
            )
        })?;
    }
    if let Some(model) = raw.plan_model {
        let model = model.trim();
        if model.is_empty() || model.contains(char::is_whitespace) {
            return Err(error(
                "plan_model",
                "plan_model must be one word, like \"sonnet\" or \"opus\"",
            ));
        }
        settings.model = model.to_string();
    }
    if let Some(t) = raw.drift_threshold {
        if !(t.is_finite() && t > 0.0 && t <= 1.0) {
            return Err(error(
                "drift_threshold",
                "drift_threshold must be above 0 and at most 1 (0.3 flags a change 30% off the plan)",
            ));
        }
        settings.drift_threshold = t;
    }
    Ok(settings)
}
