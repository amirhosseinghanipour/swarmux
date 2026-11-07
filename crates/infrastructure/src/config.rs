//! `config.toml`: user settings. A missing file means defaults.

use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::IgnoredAny;
use swarmux_application::{
    GuardSettings, JevProvider, JevSettings, LayoutSettings, MotionSetting, ReviewSettings,
    ReviewWhen, RowSettings, Settings, ThemeDef, UiSettings,
};
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
    jev: Option<RawJev>,
    review: Option<RawReview>,
    telemetry: Option<crate::config_telemetry::RawTelemetry>,
    pricing: Option<crate::config_telemetry::RawPricing>,
    worktree: Option<crate::config_worktree::RawWorktree>,
    governor: Option<crate::config_governor::RawGovernor>,
    notify: Option<crate::config_notify::RawNotify>,
    watchdog: Option<crate::config_watchdog::RawWatchdog>,
    missions: Option<crate::config_missions::RawMissions>,
    board: Option<crate::config_board::RawBoard>,
    followups: Option<crate::config_followups::RawFollowups>,
    theme: Option<String>,
    layout: Option<String>,
    icons: Option<String>,
    font: Option<String>,
    bg: Option<String>,
    themes: Option<std::collections::BTreeMap<String, toml::Value>>,
    gaps: Option<i64>,
    fps: Option<i64>,
    low_power: Option<bool>,
    motion: Option<String>,
    splash: Option<bool>,
    layouts: Option<Vec<RawLayout>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLayout {
    name: String,
    rows: Vec<RawRow>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRow {
    panels: Vec<String>,
    height: Option<i64>,
    weight: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawReview {
    enabled: Option<bool>,
    model: Option<String>,
    when: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawJev {
    provider: Option<String>,
    threshold: Option<f64>,
    timeout_ms: Option<i64>,
    /// Only to refuse it with a clear message; its value is never read.
    api_key: Option<IgnoredAny>,
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
    if let Some(jev) = raw.jev {
        settings.jev = parse_jev(jev, text, path)?;
    }
    if let Some(review) = raw.review {
        settings.review = parse_review(review, text, path)?;
    }
    if let Some(telemetry) = raw.telemetry {
        settings.telemetry = crate::config_telemetry::parse_telemetry(telemetry, text, path)?;
    }
    if let Some(pricing) = raw.pricing {
        settings.pricing = crate::config_telemetry::parse_pricing(pricing, text, path)?;
    }
    if let Some(worktree) = raw.worktree {
        settings.worktree = crate::config_worktree::parse_worktree(worktree, text, path)?;
    }
    if let Some(governor) = raw.governor {
        settings.governor = crate::config_governor::parse_governor(governor, text, path)?;
    }
    if let Some(notify) = raw.notify {
        settings.notify = crate::config_notify::parse_notify(notify, text, path)?;
    }
    if let Some(watchdog) = raw.watchdog {
        settings.watchdog = crate::config_watchdog::parse_watchdog(watchdog, text, path)?;
    }
    if let Some(board) = raw.board {
        settings.board = crate::config_board::parse_board(board);
    }
    if let Some(missions) = raw.missions {
        settings.missions = crate::config_missions::parse_missions(missions, text, path)?;
    }
    if let Some(followups) = raw.followups {
        settings.followups = crate::config_followups::parse_followups(followups, text, path)?;
    }
    // The watchdog calls a loop at the guard's count; the loop breaker then
    // stops the job one failure later.
    settings.watchdog.limits.loop_repeats = settings.guard.loop_repeats;
    let ui = RawUi {
        theme: raw.theme,
        layout: raw.layout,
        icons: raw.icons,
        font: raw.font,
        bg: raw.bg,
        themes: raw.themes,
        gaps: raw.gaps,
        fps: raw.fps,
        low_power: raw.low_power,
        motion: raw.motion,
        splash: raw.splash,
        layouts: raw.layouts,
    };
    settings.ui = parse_ui(ui, text, path)?;
    Ok(settings)
}

/// The look-and-feel keys of [`RawConfig`].
struct RawUi {
    theme: Option<String>,
    layout: Option<String>,
    icons: Option<String>,
    font: Option<String>,
    bg: Option<String>,
    themes: Option<std::collections::BTreeMap<String, toml::Value>>,
    gaps: Option<i64>,
    fps: Option<i64>,
    low_power: Option<bool>,
    motion: Option<String>,
    splash: Option<bool>,
    layouts: Option<Vec<RawLayout>>,
}

fn parse_ui(raw: RawUi, text: &str, path: &Path) -> Result<UiSettings, ConfigError> {
    let error = |key: &str, message: String| ConfigError {
        path: path.to_path_buf(),
        line: key_line(text, key),
        message,
    };
    let in_range = |key: &str, v: i64, lo: u16, hi: u16| {
        u16::try_from(v)
            .ok()
            .filter(|v| (lo..=hi).contains(v))
            .ok_or_else(|| error(key, format!("{key} must be between {lo} and {hi}")))
    };
    let name = |key: &str, v: String| {
        let v = v.trim().to_string();
        if v.is_empty() {
            Err(error(key, format!("{key} can't be empty")))
        } else {
            Ok(v)
        }
    };
    let mut ui = UiSettings::default();
    if let Some(theme) = raw.theme {
        ui.theme = name("theme", theme)?;
    }
    if let Some(layout) = raw.layout {
        ui.layout = name("layout", layout)?;
    }
    if let Some(icons) = raw.icons {
        let icons = icons.trim().to_ascii_lowercase();
        if !matches!(icons.as_str(), "ascii" | "unicode" | "nerd" | "auto") {
            return Err(error(
                "icons",
                "icons must be \"unicode\", \"ascii\", \"nerd\" or \"auto\"".into(),
            ));
        }
        ui.icons = icons;
    }
    if let Some(font) = raw.font {
        ui.font = font.trim().to_string();
    }
    if let Some(bg) = raw.bg {
        ui.transparent = match bg.trim().to_ascii_lowercase().as_str() {
            "none" | "transparent" => true,
            "theme" | "solid" => false,
            _ => {
                return Err(error(
                    "bg",
                    "bg must be \"none\" (transparent) or \"theme\"".into(),
                ));
            }
        };
    }
    for (theme_name, value) in raw.themes.unwrap_or_default() {
        ui.themes.push(parse_theme_def(theme_name, value, &error)?);
    }
    if let Some(gaps) = raw.gaps {
        ui.gaps = in_range("gaps", gaps, 0, 4)?;
    }
    if let Some(fps) = raw.fps {
        ui.fps = in_range("fps", fps, 1, 30)?;
    }
    ui.low_power = raw.low_power.unwrap_or(ui.low_power);
    if let Some(motion) = raw.motion {
        ui.motion = MotionSetting::parse(&motion).ok_or_else(|| {
            error(
                "motion",
                "motion must be \"auto\", \"full\", \"reduced\" or \"off\"".into(),
            )
        })?;
    }
    ui.splash = raw.splash.unwrap_or(ui.splash);
    for layout in raw.layouts.unwrap_or_default() {
        let name = name("name", layout.name)?;
        if layout.rows.is_empty() || layout.rows.iter().any(|r| r.panels.is_empty()) {
            return Err(error(
                "rows",
                format!("layout \"{name}\" needs rows, each with at least one panel"),
            ));
        }
        let rows = layout
            .rows
            .into_iter()
            .map(|r| {
                Ok(RowSettings {
                    panels: r.panels.into_iter().map(|p| p.trim().to_string()).collect(),
                    height: r
                        .height
                        .map(|h| in_range("height", h, 1, 200))
                        .transpose()?,
                    weight: r
                        .weight
                        .map(|w| in_range("weight", w, 1, 100))
                        .transpose()?,
                })
            })
            .collect::<Result<_, ConfigError>>()?;
        ui.layouts.push(LayoutSettings { name, rows });
    }
    Ok(ui)
}

/// One `[themes.<name>]` table: `base` plus colours by role.
fn parse_theme_def(
    name: String,
    value: toml::Value,
    error: &dyn Fn(&str, String) -> ConfigError,
) -> Result<ThemeDef, ConfigError> {
    let toml::Value::Table(table) = value else {
        return Err(error("themes", format!("themes.{name} must be a table")));
    };
    let mut def = ThemeDef {
        name: name.clone(),
        base: None,
        roles: std::collections::BTreeMap::new(),
    };
    for (key, v) in table {
        if key == "base" {
            let toml::Value::String(base) = v else {
                return Err(error(&key, format!("themes.{name}.base must be a name")));
            };
            def.base = Some(base.trim().to_string());
            continue;
        }
        let Some(arity) = swarmux_application::role_arity(&key) else {
            return Err(error(
                &key,
                format!("themes.{name}: unknown role \"{key}\""),
            ));
        };
        let texts: Vec<&str> = match &v {
            toml::Value::String(s) => vec![s.as_str()],
            toml::Value::Array(items) => items.iter().filter_map(toml::Value::as_str).collect(),
            _ => Vec::new(),
        };
        let colours: Option<Vec<u32>> = texts
            .iter()
            .map(|t| swarmux_application::parse_hex(t))
            .collect();
        match colours {
            Some(c) if c.len() == arity && texts.len() == arity => {
                def.roles.insert(key, c);
            }
            _ => {
                return Err(error(
                    &key,
                    format!("themes.{name}.{key} needs {arity} colour(s) like \"#1e1e2e\""),
                ));
            }
        }
    }
    Ok(def)
}

fn parse_review(raw: RawReview, text: &str, path: &Path) -> Result<ReviewSettings, ConfigError> {
    let mut review = ReviewSettings::default();
    if let Some(enabled) = raw.enabled {
        review.enabled = enabled;
    }
    if let Some(model) = raw.model {
        let model = model.trim();
        if model.is_empty() || model.contains(char::is_whitespace) {
            return Err(ConfigError {
                path: path.to_path_buf(),
                line: key_line(text, "model"),
                message: "review model must be one word, like \"opus\" or \"haiku\"".into(),
            });
        }
        review.model = model.to_string();
    }
    if let Some(when) = raw.when {
        review.when = ReviewWhen::parse(&when).ok_or_else(|| ConfigError {
            path: path.to_path_buf(),
            line: key_line(text, "when"),
            message: "review when must be \"risk\" or \"always\"".into(),
        })?;
    }
    Ok(review)
}

fn parse_jev(raw: RawJev, text: &str, path: &Path) -> Result<JevSettings, ConfigError> {
    let error = |key: &str, message: &str| ConfigError {
        path: path.to_path_buf(),
        line: key_line(text, key),
        message: message.to_string(),
    };
    if raw.api_key.is_some() {
        return Err(error(
            "api_key",
            "keys aren't read from config.toml: run `swarmux jev set-key <provider>`              or set TYPESAFE_API_KEY / OPENROUTER_API_KEY",
        ));
    }
    let mut jev = JevSettings::default();
    if let Some(p) = raw.provider {
        jev.provider = JevProvider::parse(p.trim()).ok_or_else(|| {
            error(
                "provider",
                "provider must be \"off\", \"typesafe\" or \"openrouter\"",
            )
        })?;
    }
    if let Some(t) = raw.threshold {
        if !(0.0..=1.0).contains(&t) {
            return Err(error("threshold", "threshold must be between 0 and 1"));
        }
        jev.threshold = t;
    }
    if let Some(ms) = raw.timeout_ms {
        jev.timeout_ms = Some(
            u64::try_from(ms)
                .ok()
                .filter(|ms| (100..=60_000).contains(ms))
                .ok_or_else(|| error("timeout_ms", "timeout_ms must be between 100 and 60000"))?,
        );
    }
    Ok(jev)
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
pub(crate) fn key_line(text: &str, key: &str) -> Option<usize> {
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
    fn worktree_section_overrides_and_is_checked() {
        let s = parse(
            "[worktree]\nroot = \"D:/wt\"\ncopy = [\".env\", \"config/*.local\"]\nport_base = 40000\nport_block = 10\nport_slots = 100\nstale_hours = 6\n",
        )
        .unwrap();
        assert_eq!(s.worktree.root, Some(PathBuf::from("D:/wt")));
        assert_eq!(s.worktree.copy, [".env", "config/*.local"]);
        assert_eq!(s.worktree.ports.port_base(1), Some(40_010));
        assert_eq!(s.worktree.stale_hours, 6);
        for bad in [
            "[worktree]\ncopy = [\"../secrets\"]\n",
            "[worktree]\ncopy = [\"/etc/passwd\"]\n",
            "[worktree]\nport_base = 80\n",
            "[worktree]\nport_base = 65000\nport_block = 100\nport_slots = 50\n",
            "[worktree]\nstale_hours = 0\n",
            "[worktree]\nunknown = 1\n",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
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
    fn jev_section_is_read() {
        let s = parse(
            "[jev]
provider = \"openrouter\"
threshold = 0.9
timeout_ms = 3000
",
        )
        .unwrap();
        assert_eq!(
            s.jev,
            JevSettings {
                provider: JevProvider::OpenRouter,
                threshold: 0.9,
                timeout_ms: Some(3000),
            }
        );
        assert_eq!(parse("").unwrap().jev.provider, JevProvider::Off);
        let only = parse(
            "[jev]
provider = \"typesafe\"",
        )
        .unwrap()
        .jev;
        assert_eq!((only.threshold, only.timeout_ms()), (0.85, 2000));
    }

    #[test]
    fn bad_jev_values_are_rejected() {
        let e = parse(
            "[jev]
provider = \"gpt\"",
        )
        .unwrap_err();
        assert_eq!(e.line, Some(2));
        assert!(e.message.contains("typesafe"), "{e}");
        assert!(
            parse(
                "[jev]
threshold = 1.5"
            )
            .is_err()
        );
        assert!(
            parse(
                "[jev]
timeout_ms = 0"
            )
            .is_err()
        );
        assert!(
            parse(
                "[jev]
model = \"x\""
            )
            .is_err()
        );
    }

    #[test]
    fn keys_in_config_are_refused_without_echoing_them() {
        let e = parse(
            "[jev]
provider = \"typesafe\"
api_key = \"sk-SECRET-123\"",
        )
        .unwrap_err();
        assert_eq!(e.line, Some(3));
        assert!(e.message.contains("swarmux jev set-key"), "{e}");
        assert!(!e.to_string().contains("SECRET"), "{e}");
    }

    #[test]
    fn board_section_is_read_and_on_by_default() {
        assert!(parse("").unwrap().board.enabled);
        assert!(
            !parse(
                "[board]
enabled = false
"
            )
            .unwrap()
            .board
            .enabled
        );
        assert!(
            parse(
                "[board]
enabled = true
"
            )
            .unwrap()
            .board
            .enabled
        );
        assert!(
            parse(
                "[board]
enabled = \"yes\"
"
            )
            .is_err()
        );
        assert!(
            parse(
                "[board]
unknown = 1
"
            )
            .is_err()
        );
    }

    #[test]
    fn followups_section_reads_times_and_standing_orders() {
        let s = parse(
            "[followups]\nbrief_times = [\"08:30\", \"17:00\"]\nextract_model = \"haiku\"\n\n\
             [[followups.order]]\nid = \"docs\"\nkinds = [\"task\"]\npaths = [\"docs/**\"]\n\
             action = \"auto-approve\"\n\n[[followups.order]]\nkeywords = [\"migration\"]\naction = \"always-ask\"\n",
        )
        .unwrap()
        .followups;
        assert_eq!(s.slots.labels(), ["08:30", "17:00"]);
        assert_eq!(s.orders.len(), 2);
        assert_eq!(s.orders[0].id, "docs");
        assert_eq!(s.orders[1].id, "order-2");
        assert!(!s.orders[0].allow_risky);
        assert_eq!(parse("").unwrap().followups.slots.labels().len(), 3);
        assert!(parse("[followups]\nbrief_times = [\"9am\"]\n").is_err());
        assert!(parse("[[followups.order]]\naction = \"auto-approve\"\n").is_err());
        assert!(parse("[[followups.order]]\nkinds = [\"nope\"]\naction = \"park\"\n").is_err());
    }

    #[test]
    fn missions_section_is_read_and_checked() {
        let s = parse(
            "[missions]\nplan_first = true\nplan_model = \" opus \"\ndrift_threshold = 0.5\n",
        )
        .unwrap();
        assert!(s.missions.plan_first);
        assert_eq!(s.missions.model, "opus");
        assert_eq!(s.missions.drift_threshold, 0.5);
        assert!(!parse("").unwrap().missions.plan_first);
        assert!(parse("").unwrap().missions.parallel, "parallel by default");
        assert!(
            !parse("[missions]\nparallel = false\n")
                .unwrap()
                .missions
                .parallel
        );
        for bad in [
            "[missions]\nplan_model = \"\"\n",
            "[missions]\ndrift_threshold = 0\n",
            "[missions]\ndrift_threshold = 1.5\n",
            "[missions]\nplan_first = \"yes\"\n",
            "[missions]\nunknown = 1\n",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn watchdog_section_overrides_and_is_checked() {
        let s = parse(
            "[guard]
loop_repeats = 3

[watchdog]
rungs = [\"nudge\", \"fresh\", \"inbox\"]
stall_secs = 90
stall_tool_secs = 1200
stall_turns = 20
oscillation_repeats = 4
context_high = 0.9
compactions = 3
burn_usd_per_min = 2.5
nudge_cooldown_secs = 60
restart_cooldown_secs = 300
max_restarts = 1
escalate_to = [\"opus\"]
",
        )
        .unwrap();
        let w = &s.watchdog;
        assert!(w.enabled);
        assert_eq!(
            w.ladder.rungs,
            [
                swarmux_domain::Rung::Nudge,
                swarmux_domain::Rung::Fresh,
                swarmux_domain::Rung::Inbox
            ]
        );
        assert_eq!(
            (w.limits.stall_silent_ms, w.limits.stall_tool_ms),
            (90_000, 1_200_000)
        );
        assert_eq!(
            (w.limits.stall_turns, w.limits.oscillation_repeats),
            (20, 4)
        );
        assert_eq!((w.limits.context_high, w.limits.compactions_high), (0.9, 3));
        assert_eq!(w.limits.burn_usd_per_min, 2.5);
        assert_eq!(w.limits.loop_repeats, 3, "follows the guard");
        assert_eq!(
            (w.ladder.nudge_cooldown_ms, w.ladder.restart_cooldown_ms),
            (60_000, 300_000)
        );
        assert_eq!(w.ladder.max_restarts, 1);
        assert_eq!(w.escalate_to, ["opus"]);
        assert_eq!(
            parse("").unwrap().watchdog,
            swarmux_application::WatchdogSettings::default()
        );
        assert!(
            !parse("[watchdog]\nenabled = false\n")
                .unwrap()
                .watchdog
                .enabled
        );
        for (bad, why) in [
            ("rungs = [\"pray\"]", "unknown rung"),
            ("stall_secs = 5", "between 30 and 3600"),
            ("context_high = 1.5", "context_high"),
            ("burn_usd_per_min = -1.0", "burn_usd_per_min"),
            ("max_restarts = 99", "max_restarts"),
            ("escalate_to = [\"big model\"]", "escalate_to"),
            ("nap = true", "unknown field"),
        ] {
            let e = parse(&format!("[watchdog]\n{bad}\n")).unwrap_err();
            assert!(e.to_string().contains(why), "{bad}: {e}");
        }
    }

    #[test]
    fn governor_section_overrides_and_is_checked() {
        let s = parse(
            "[governor]
enabled = true
hold_at = 0.95
downgrade_at = 0.8
downgrade_to = \"haiku\"
horizon_minutes = 45
battery_concurrency = 1
max_job_usd = 4.5
budget_line = false
",
        )
        .unwrap();
        let g = &s.governor;
        assert!(g.enabled && !g.budget_line);
        assert_eq!((g.hold_at, g.downgrade_at), (0.95, 0.8));
        assert_eq!(g.downgrade_to, "haiku");
        assert_eq!((g.horizon_minutes, g.battery_concurrency), (45, 1));
        assert_eq!(g.max_job_usd, Some(4.5));
        assert_eq!(
            parse("").unwrap().governor,
            swarmux_application::GovernorSettings::default()
        );
        assert_eq!(
            parse(
                "[governor]
hold_at = 0.97
"
            )
            .unwrap()
            .governor
            .hold_at,
            0.97
        );
        assert_eq!(parse("").unwrap().governor.load_hold_at, 0.70);
        assert_eq!(
            parse(
                "[governor]
load_hold_at = 0.8
"
            )
            .unwrap()
            .governor
            .load_hold_at,
            0.8
        );
        assert!(
            parse(
                "[governor]
load_hold_at = 0
"
            )
            .is_err()
        );
        for bad in [
            "[governor]
hold_at = 97
",
            "[governor]
hold_at = 0
",
            "[governor]
downgrade_at = 1.5
",
            "[governor]
downgrade_to = \"\"
",
            "[governor]
horizon_minutes = 0
",
            "[governor]
battery_concurrency = 0
",
            "[governor]
max_job_usd = -1.0
",
            "[governor]
unknown = 1
",
        ] {
            let e = parse(bad).unwrap_err();
            assert!(bad.contains("unknown") || e.line == Some(2), "{bad}: {e}");
        }
    }

    #[test]
    fn notify_section_is_read_and_checked() {
        let s = parse(
            "[notify]
enabled = false
toast = false
terminal = \"osc9\"
quiet = \"22:00-08:00\"
ntfy = \"https://ntfy.sh/my-topic\"
",
        )
        .unwrap();
        let n = &s.notify;
        assert!(!n.enabled && !n.toast);
        assert_eq!(n.terminal, swarmux_application::TerminalMode::Osc9);
        assert!(n.quiet.is_some());
        assert_eq!(n.ntfy.as_deref(), Some("https://ntfy.sh/my-topic"));
        assert_eq!(parse("").unwrap().notify.ntfy, None);
        for bad in [
            "[notify]\nquiet = \"late\"\n",
            "[notify]\nterminal = \"loud\"\n",
            "[notify]\nntfy = \"ntfy.sh/topic\"\n",
            "[notify]\nntfy = \"https://ntfy.sh/a b\"\n",
        ] {
            assert_eq!(parse(bad).unwrap_err().line, Some(2), "{bad}");
        }
    }

    #[test]
    fn review_section_is_read() {
        let s = parse("[review]\nenabled = true\nmodel = \" haiku \"\n").unwrap();
        assert_eq!(
            s.review,
            ReviewSettings {
                enabled: true,
                model: "haiku".into(),
                when: ReviewWhen::Risk,
            }
        );
        assert_eq!(parse("").unwrap().review, ReviewSettings::default());
        let e = parse("[review]\nmodel = \"\"").unwrap_err();
        assert_eq!(e.line, Some(2));
        assert!(parse("[review]\nenabled = \"yes\"").is_err());
        assert!(parse("[review]\nreviewer = \"x\"").is_err());
        let always = parse("[review]\nwhen = \"always\"\n").unwrap();
        assert_eq!(always.review.when, ReviewWhen::Always);
        let e = parse("[review]\nenabled = true\nwhen = \"sometimes\"\n").unwrap_err();
        assert_eq!(e.line, Some(3));
    }

    #[test]
    fn motion_defaults_to_auto_and_rejects_unknown_tiers() {
        assert_eq!(parse("").unwrap().ui.motion, MotionSetting::Auto);
        let off = parse("motion = \"off\"\n").unwrap();
        assert_eq!(off.ui.motion, MotionSetting::Off);
        let e = parse("motion = \"wild\"\n").unwrap_err();
        assert_eq!(e.line, Some(1));
    }

    #[test]
    fn look_and_feel_keys_are_read() {
        let s = parse(
            "theme = \"tokyo-night\"\nlayout = \"btop\"\nicons = \"Nerd\"\ngaps = 0\nfps = 24\n\
             low_power = true\nmotion = \"Reduced\"\nsplash = false\n\n[[layouts]]\nname = \"focus\"\n\
             rows = [{ panels = [\"lead:2\", \"guard\"], height = 6 }, { panels = [\"log\"] }]\n",
        )
        .unwrap();
        let ui = s.ui;
        assert_eq!(
            (ui.theme.as_str(), ui.layout.as_str()),
            ("tokyo-night", "btop")
        );
        assert_eq!((ui.icons.as_str(), ui.gaps, ui.fps), ("nerd", 0, 24));
        assert!(ui.low_power && !ui.splash);
        assert_eq!(ui.motion, MotionSetting::Reduced);
        assert_eq!(
            ui.layouts,
            [LayoutSettings {
                name: "focus".into(),
                rows: vec![
                    RowSettings {
                        panels: vec!["lead:2".into(), "guard".into()],
                        height: Some(6),
                        weight: None,
                    },
                    RowSettings {
                        panels: vec!["log".into()],
                        height: None,
                        weight: None,
                    },
                ],
            }]
        );
        assert_eq!(parse("").unwrap().ui, UiSettings::default());
    }

    #[test]
    fn rice_keys_bg_icons_font_and_user_themes() {
        let s = parse(
            "theme = \"mine\"
bg = \"none\"
icons = \"auto\"
font = \"JetBrainsMono NF\"

             [themes.mine]
base = \"nord\"
lead = \"#112233\"
gauge = [\"#00ff00\", \"#ffff00\", \"#ff0000\"]
",
        )
        .unwrap();
        let ui = s.ui;
        assert!(ui.transparent);
        assert_eq!(
            (ui.icons.as_str(), ui.font.as_str()),
            ("auto", "JetBrainsMono NF")
        );
        assert_eq!(ui.themes.len(), 1);
        let def = &ui.themes[0];
        assert_eq!(
            (def.name.as_str(), def.base.as_deref()),
            ("mine", Some("nord"))
        );
        assert_eq!(def.roles["lead"], [0x112233]);
        assert_eq!(def.roles["gauge"], [0x00ff00, 0xffff00, 0xff0000]);
        assert!(!parse("bg = \"theme\"").unwrap().ui.transparent);
        assert_eq!(
            parse("").unwrap().ui.icons,
            "unicode",
            "unicode is the default"
        );
    }

    #[test]
    fn bad_rice_values_are_refused() {
        assert!(parse("bg = \"blue\"").is_err());
        assert!(
            parse(
                "[themes.x]
nope = \"#000000\""
            )
            .is_err()
        );
        assert!(
            parse(
                "[themes.x]
bg = \"red\""
            )
            .is_err()
        );
        assert!(
            parse(
                "[themes.x]
gauge = [\"#000000\"]"
            )
            .is_err()
        );
        assert!(
            parse(
                "[themes.x]
bg = [\"#000000\", \"#111111\"]"
            )
            .is_err()
        );
    }

    #[test]
    fn bad_look_and_feel_values_name_the_line() {
        let e = parse("theme = \"x\"\nfps = 60").unwrap_err();
        assert_eq!(e.line, Some(2));
        assert!(e.message.contains("between 1 and 30"), "{e}");
        assert_eq!(parse("icons = \"Unicode\"").unwrap().ui.icons, "unicode");
        assert!(parse("icons = \"emoji\"").is_err());
        assert!(parse("gaps = 9").is_err());
        assert!(parse("theme = \" \"").is_err());
        assert!(parse("[[layouts]]\nname = \"x\"\nrows = []").is_err());
        assert!(parse("[[layouts]]\nname = \"x\"\nrows = [{ panels = [] }]").is_err());
        assert!(
            parse("[[layouts]]\nname = \"x\"\nrows = [{ panels = [\"a\"], height = 0 }]").is_err()
        );
    }

    #[test]
    fn unreadable_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let e = load_settings(dir.path()).unwrap_err();
        assert_eq!(e.line, None);
        assert!(e.to_string().contains(&dir.path().display().to_string()));
    }
}
