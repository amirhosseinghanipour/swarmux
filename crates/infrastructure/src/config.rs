//! `config.toml`: user settings. A missing file means defaults.

use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use swarmux_application::{GuardSettings, Settings};
use swarmux_domain::{Pattern, Unmatched};

/// Why the config file couldn't be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub path: PathBuf,
    /// 1-based line the problem is on, when known.
    pub line: Option<usize>,
    pub message: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "{} line {line}: {}", self.path.display(), self.message),
            None => write!(f, "{}: {}", self.path.display(), self.message),
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    max_concurrent: Option<i64>,
    min_free_ram_mb: Option<i64>,
    default_model: Option<String>,
    guard: Option<RawGuard>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGuard {
    allow: Option<Vec<String>>,
    deny: Option<Vec<String>>,
    unmatched: Option<String>,
    approval_timeout_secs: Option<i64>,
    loop_repeats: Option<i64>,
}

/// Reads settings from `path`; a missing file gives the defaults.
pub fn load_settings(path: &Path) -> Result<Settings, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_settings(&text, path),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(ConfigError {
            path: path.to_path_buf(),
            line: None,
            message: e.to_string(),
        }),
    }
}

/// Parses settings from `text`; `path` only names the file in errors.
pub fn parse_settings(text: &str, path: &Path) -> Result<Settings, ConfigError> {
    let error = |line: Option<usize>, message: String| ConfigError {
        path: path.to_path_buf(),
        line,
        message,
    };
    let raw: RawConfig = toml::from_str(text).map_err(|e| {
        let line = e.span().map(|span| line_at(text, span.start));
        error(line, e.message().trim().to_string())
    })?;

    let mut settings = Settings::default();
    if let Some(n) = raw.max_concurrent {
        settings.max_concurrent = usize::try_from(n).ok().filter(|&n| n >= 1).ok_or_else(|| {
            error(
                key_line(text, "max_concurrent"),
                "max_concurrent must be at least 1".into(),
            )
        })?;
    }
    if let Some(mb) = raw.min_free_ram_mb {
        settings.min_free_ram_mb = u64::try_from(mb).map_err(|_| {
            error(
                key_line(text, "min_free_ram_mb"),
                "min_free_ram_mb can't be negative".into(),
            )
        })?;
    }
    if let Some(model) = raw.default_model {
        let model = model.trim();
        if model.is_empty() {
            return Err(error(
                key_line(text, "default_model"),
                "default_model can't be empty".into(),
            ));
        }
        settings.default_model = model.to_string();
    }
    if let Some(guard) = raw.guard {
        settings.guard = parse_guard(guard, text, path)?;
    }
    Ok(settings)
}

fn parse_guard(raw: RawGuard, text: &str, path: &Path) -> Result<GuardSettings, ConfigError> {
    let error = |line: Option<usize>, message: String| ConfigError {
        path: path.to_path_buf(),
        line,
        message,
    };
    let rules = |list: Option<Vec<String>>| -> Result<Vec<Pattern>, ConfigError> {
        list.unwrap_or_default()
            .iter()
            .map(|r| {
                Pattern::parse(r).map_err(|e| {
                    let quoted = [format!("\"{r}\""), format!("'{r}'")];
                    let line = text
                        .lines()
                        .position(|l| quoted.iter().any(|q| l.contains(q.as_str())));
                    error(line.map(|i| i + 1), e.to_string())
                })
            })
            .collect()
    };
    let mut guard = GuardSettings {
        allow: rules(raw.allow)?,
        deny: rules(raw.deny)?,
        ..GuardSettings::default()
    };
    if let Some(u) = raw.unmatched {
        guard.unmatched = match u.trim() {
            "ask" => Unmatched::Ask,
            "pass" => Unmatched::Pass,
            _ => {
                return Err(error(
                    key_line(text, "unmatched"),
                    "unmatched must be \"ask\" or \"pass\"".into(),
                ));
            }
        };
    }
    if let Some(secs) = raw.approval_timeout_secs {
        guard.approval_timeout_secs = u64::try_from(secs)
            .ok()
            .filter(|&s| (1..=3600).contains(&s))
            .ok_or_else(|| {
                error(
                    key_line(text, "approval_timeout_secs"),
                    "approval_timeout_secs must be between 1 and 3600".into(),
                )
            })?;
    }
    if let Some(n) = raw.loop_repeats {
        guard.loop_repeats = u32::try_from(n).ok().filter(|&n| n != 1).ok_or_else(|| {
            error(
                key_line(text, "loop_repeats"),
                "loop_repeats must be 0 (off) or at least 2".into(),
            )
        })?;
    }
    Ok(guard)
}

/// 1-based line of byte offset `at`.
fn line_at(text: &str, at: usize) -> usize {
    let at = at.min(text.len());
    text.as_bytes()[..at]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

/// 1-based line where `key` is set.
fn key_line(text: &str, key: &str) -> Option<usize> {
    text.lines()
        .position(|l| {
            l.trim_start()
                .strip_prefix(key)
                .is_some_and(|rest| rest.trim_start().starts_with('='))
        })
        .map(|i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Settings, ConfigError> {
        parse_settings(text, Path::new("config.toml"))
    }

    #[test]
    fn empty_file_gives_defaults() {
        assert_eq!(parse("").unwrap(), Settings::default());
    }

    #[test]
    fn values_override_defaults() {
        let s = parse(
            "# mine\nmax_concurrent = 5\nmin_free_ram_mb = 512\ndefault_model = \" opus \"\n",
        )
        .unwrap();
        assert_eq!(
            s,
            Settings {
                max_concurrent: 5,
                min_free_ram_mb: 512,
                default_model: "opus".into(),
                ..Settings::default()
            }
        );
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let s = load_settings(&dir.path().join("config.toml")).unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn syntax_error_names_file_and_line() {
        let e = parse("max_concurrent = 2\nmin_free_ram_mb = \n").unwrap_err();
        assert_eq!(e.line, Some(2));
        let shown = e.to_string();
        assert!(shown.starts_with("config.toml line 2: "), "{shown}");
    }

    #[test]
    fn wrong_type_and_unknown_key_are_reported_with_line() {
        let e = parse("\nmax_concurrent = \"three\"\n").unwrap_err();
        assert_eq!(e.line, Some(2), "{e}");
        let e = parse("max_concurent = 3\n").unwrap_err();
        assert_eq!(e.line, Some(1));
        assert!(e.message.contains("max_concurent"), "{e}");
    }

    #[test]
    fn out_of_range_values_are_rejected() {
        let e = parse("\n\nmax_concurrent = 0").unwrap_err();
        assert_eq!(e.line, Some(3));
        assert!(e.message.contains("at least 1"));
        assert!(parse("min_free_ram_mb = -1").is_err());
        assert!(parse("default_model = \"  \"").is_err());
        assert_eq!(parse("min_free_ram_mb = 0").unwrap().min_free_ram_mb, 0);
    }

    #[test]
    fn guard_section_is_read() {
        let s = parse(
            "[guard]
allow = [\"Bash(cargo test:*)\", \"Edit(src/**)\"]
deny = [\"WebFetch\"]
unmatched = \"pass\"
approval_timeout_secs = 30
loop_repeats = 0
",
        )
        .unwrap();
        let g = s.guard;
        let shown: Vec<String> = g.allow.iter().map(ToString::to_string).collect();
        assert_eq!(shown, ["Bash(cargo test:*)", "Edit(src/**)"]);
        assert_eq!(g.deny[0].to_string(), "WebFetch");
        assert_eq!(g.unmatched, Unmatched::Pass);
        assert_eq!((g.approval_timeout_secs, g.loop_repeats), (30, 0));
    }

    #[test]
    fn guard_defaults_without_section() {
        assert_eq!(
            parse("max_concurrent = 2").unwrap().guard,
            GuardSettings::default()
        );
    }

    #[test]
    fn bad_rules_are_reported_with_line() {
        let e = parse(
            "[guard]
allow = [
  \"Bash(ok)\",
  \"Bash(\",
]
",
        )
        .unwrap_err();
        assert_eq!(e.line, Some(4), "{e}");
        assert!(e.message.contains("Bash("), "{e}");
    }

    #[test]
    fn bad_guard_values_are_rejected() {
        assert!(
            parse(
                "[guard]
unmatched = \"maybe\""
            )
            .is_err()
        );
        assert!(
            parse(
                "[guard]
approval_timeout_secs = 0"
            )
            .is_err()
        );
        assert!(
            parse(
                "[guard]
loop_repeats = 1"
            )
            .is_err()
        );
        let e = parse(
            "[guard]
alow = []",
        )
        .unwrap_err();
        assert!(e.message.contains("alow"), "{e}");
    }

    #[test]
    fn unreadable_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let e = load_settings(dir.path()).unwrap_err();
        assert_eq!(e.line, None);
        assert!(e.to_string().contains(&dir.path().display().to_string()));
    }
}
