//! The `[telemetry]` and `[pricing]` sections of `config.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use swarmux_application::TelemetrySettings;
use swarmux_domain::telemetry::{ModelPrice, PriceTable};

use crate::config::ConfigError;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawTelemetry {
    usage: Option<bool>,
    system: Option<bool>,
    git: Option<bool>,
    github: Option<bool>,
    agents: Option<bool>,
    usage_secs: Option<i64>,
    system_secs: Option<i64>,
    git_secs: Option<i64>,
    github_secs: Option<i64>,
    agents_secs: Option<i64>,
    transcripts_path: Option<String>,
}

/// USD per million tokens; cache prices default from the input price.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawPrice {
    input: f64,
    output: f64,
    cache_write_5m: Option<f64>,
    cache_write_1h: Option<f64>,
    cache_read: Option<f64>,
}

pub(crate) type RawPricing = BTreeMap<String, RawPrice>;

/// Longest allowed interval between two collections, in seconds.
const MAX_SECS: i64 = 3600;

pub(crate) fn parse_telemetry(
    raw: RawTelemetry,
    text: &str,
    path: &Path,
) -> Result<TelemetrySettings, ConfigError> {
    let mut t = TelemetrySettings::default();
    let on = |v: Option<bool>, default: bool| v.unwrap_or(default);
    t.usage = on(raw.usage, t.usage);
    t.system = on(raw.system, t.system);
    t.git = on(raw.git, t.git);
    t.github = on(raw.github, t.github);
    t.agents = on(raw.agents, t.agents);
    let secs = |key: &str, v: Option<i64>, default: u64| -> Result<u64, ConfigError> {
        match v {
            None => Ok(default),
            Some(n) if (1..=MAX_SECS).contains(&n) => Ok(n as u64),
            Some(_) => Err(ConfigError {
                path: path.to_path_buf(),
                line: crate::config::key_line(text, key),
                message: format!("{key} must be between 1 and {MAX_SECS}"),
            }),
        }
    };
    t.usage_secs = secs("usage_secs", raw.usage_secs, t.usage_secs)?;
    t.system_secs = secs("system_secs", raw.system_secs, t.system_secs)?;
    t.git_secs = secs("git_secs", raw.git_secs, t.git_secs)?;
    t.github_secs = secs("github_secs", raw.github_secs, t.github_secs)?;
    t.agents_secs = secs("agents_secs", raw.agents_secs, t.agents_secs)?;
    if let Some(p) = raw.transcripts_path {
        let p = p.trim();
        if p.is_empty() {
            return Err(ConfigError {
                path: path.to_path_buf(),
                line: crate::config::key_line(text, "transcripts_path"),
                message: "transcripts_path can't be empty".into(),
            });
        }
        t.transcripts_path = Some(PathBuf::from(p));
    }
    Ok(t)
}

pub(crate) fn parse_pricing(
    raw: RawPricing,
    text: &str,
    path: &Path,
) -> Result<PriceTable, ConfigError> {
    let mut table = PriceTable::builtin();
    for (model, p) in raw {
        let values = [
            Some(p.input),
            Some(p.output),
            p.cache_write_5m,
            p.cache_write_1h,
            p.cache_read,
        ];
        if model.trim().is_empty() || values.iter().flatten().any(|v| !v.is_finite() || *v < 0.0) {
            let line = text
                .lines()
                .position(|l| {
                    l.contains(&format!("[pricing.\"{model}\"]"))
                        || l.contains(&format!("[pricing.{model}]"))
                })
                .map(|i| i + 1);
            return Err(ConfigError {
                path: path.to_path_buf(),
                line,
                message: format!("prices of \"{model}\" must be numbers of at least 0"),
            });
        }
        let mut price =
            ModelPrice::standard(p.input, p.output, p.cache_read.unwrap_or(p.input * 0.1));
        if let Some(v) = p.cache_write_5m {
            price.cache_write_5m = v;
        }
        if let Some(v) = p.cache_write_1h {
            price.cache_write_1h = v;
        }
        table.set(&model, price);
    }
    Ok(table)
}

#[cfg(test)]
mod tests {
    use swarmux_application::Settings;

    use crate::config::parse_settings;

    fn parse(text: &str) -> Result<Settings, crate::ConfigError> {
        parse_settings(text, std::path::Path::new("config.toml"))
    }

    #[test]
    fn telemetry_section_is_read() {
        let s = parse(
            "[telemetry]\ngithub = false\nsystem_secs = 2\nusage_secs = 30\n\
             transcripts_path = \"/data/projects\"\n",
        )
        .unwrap();
        let t = s.telemetry;
        assert!(!t.github && t.usage && t.git && t.system);
        assert_eq!((t.system_secs, t.usage_secs, t.git_secs), (2, 30, 5));
        assert_eq!(
            t.transcripts_path.as_deref(),
            Some(std::path::Path::new("/data/projects"))
        );
    }

    #[test]
    fn bad_telemetry_values_are_rejected() {
        let e = parse("[telemetry]\n\ngit_secs = 0\n").unwrap_err();
        assert_eq!(e.line, Some(3));
        assert!(e.message.contains("git_secs"), "{e}");
        assert!(parse("[telemetry]\ntranscripts_path = \" \"\n").is_err());
        assert!(parse("[telemetry]\ncpu = true\n").is_err());
    }

    #[test]
    fn pricing_overrides_builtin_prices() {
        let s = parse(
            "[pricing.\"claude-opus-5-5\"]\ninput = 1.0\noutput = 5.0\n\n\
             [pricing.my-model]\ninput = 2.0\noutput = 4.0\ncache_read = 0.5\ncache_write_1h = 3.0\n",
        )
        .unwrap();
        let opus = s.pricing.price_for("claude-opus-5-5").unwrap();
        assert_eq!((opus.input, opus.output, opus.cache_read), (1.0, 5.0, 0.1));
        assert_eq!(opus.cache_write_5m, 1.25);
        let mine = s.pricing.price_for("my-model").unwrap();
        assert_eq!((mine.cache_read, mine.cache_write_1h), (0.5, 3.0));
        // Other models keep the built-in price.
        assert_eq!(
            s.pricing.price_for("claude-sonnet-5-5").map(|p| p.input),
            Some(2.0)
        );
    }

    #[test]
    fn bad_prices_are_rejected() {
        let e = parse("\n[pricing.x]\ninput = -1.0\noutput = 1.0\n").unwrap_err();
        assert_eq!(e.line, Some(2));
        assert!(parse("[pricing.x]\ninput = 1.0\n").is_err());
        assert!(parse("[pricing.x]\ninput = 1.0\noutput = 1.0\nbogus = 1\n").is_err());
    }
}
