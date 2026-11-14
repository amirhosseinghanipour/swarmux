//! `config.toml`: user settings. A missing file means defaults.

use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use swarmux_application::{
    GuardSettings, LayoutSettings, MotionSetting, ReviewSettings, ReviewWhen, RowSettings,
    Settings, ThemeDef, UiSettings,
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
    /// The version of the file's format; read by the layering, accepted here.
    #[allow(dead_code)]
    schema_version: Option<toml::Value>,
    max_concurrent: Option<i64>,
    min_free_ram_mb: Option<i64>,
    default_model: Option<String>,
    guard: Option<RawGuard>,
    /// JEV was removed: a `[jev]` section is read as anything and ignored,
    /// with a warning, so an old config still loads.
    jev: Option<toml::Value>,
    review: Option<RawReview>,
    telemetry: Option<crate::config_telemetry::RawTelemetry>,
    pricing: Option<crate::config_telemetry::RawPricing>,
    worktree: Option<crate::config_worktree::RawWorktree>,
    governor: Option<crate::config_governor::RawGovernor>,
    notify: Option<crate::config_notify::RawNotify>,
    watchdog: Option<crate::config_watchdog::RawWatchdog>,
    missions: Option<crate::config_missions::RawMissions>,
    console: Option<crate::config_console::RawConsole>,
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
    /// Read whole: what is wrong in it is for the keymap to report, not a
    /// reason to refuse the file.
    keys: Option<toml::Value>,
    /// Read whole too: the Deck's lint reports what is wrong in it.
    deck: Option<toml::Value>,
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
struct RawGuard {
    allow: Option<Vec<String>>,
    deny: Option<Vec<String>>,
    unmatched: Option<String>,
    approval_timeout_secs: Option<i64>,
    loop_repeats: Option<i64>,
    // Read line by line: see `config_guard`.
    on_unreachable: Option<toml::Value>,
    always_scope: Option<toml::Value>,
    always_expiry_days: Option<toml::Value>,
    protected_paths: Option<toml::Value>,
    hook_on_failure_block: Option<toml::Value>,
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
    build(raw, text, path)
}

/// The settings of an already parsed (and merged) table. `text` only helps
/// to name lines in messages: it is the file the values mostly came from.
pub fn settings_from_table(
    table: &toml::Table,
    text: &str,
    path: &Path,
) -> Result<Settings, ConfigError> {
    let raw: RawConfig = table
        .clone()
        .try_into()
        .map_err(|e: toml::de::Error| ConfigError {
            path: path.to_path_buf(),
            line: None,
            message: e.message().trim().to_string(),
        })?;
    build(raw, text, path)
}

fn build(raw: RawConfig, text: &str, path: &Path) -> Result<Settings, ConfigError> {
    let error = |line: Option<usize>, message: String| ConfigError {
        path: path.to_path_buf(),
        line,
        message,
    };
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
    let mut extras = crate::config_guard::GuardExtras::default();
    if let Some(mut guard) = raw.guard {
        extras.on_unreachable = guard.on_unreachable.take();
        extras.always_scope = guard.always_scope.take();
        extras.always_expiry_days = guard.always_expiry_days.take();
        extras.protected_paths = guard.protected_paths.take();
        extras.hook_on_failure_block = guard.hook_on_failure_block.take();
        settings.guard = parse_guard(guard, text, path)?;
    }
    if let Some(jev) = &raw.jev {
        settings.warnings.push(removed_jev_warning(jev, text));
    }
    crate::config_guard::apply(extras, text, &mut settings);
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
        settings.watchdog =
            crate::config_watchdog::parse_watchdog(watchdog, text, path, &mut settings.warnings)?;
    }
    if let Some(board) = raw.board {
        settings.board = crate::config_board::parse_board(board);
    }
    if let Some(missions) = raw.missions {
        settings.missions = crate::config_missions::parse_missions(missions, text, path)?;
    }
    if let Some(console) = raw.console {
        settings.console = crate::config_console::parse_console(console, text, path)?;
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
    if let Some(keys) = &raw.keys {
        settings.keys = crate::config_keys::parse_keys(keys);
    }
    if let Some(deck) = &raw.deck {
        settings.deck = crate::config_deck::parse_deck(deck);
    }
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

// COMPAT(jev-section, 0.1.0, 2026-11-14): a `[jev]` section is read, ignored and
// named in a warning; remove after 2027-11-14.
/// What is said about an old `[jev]` section (the hosted decision model,
/// removed): that it is ignored, where it is, and what is left to do. Never
/// shows a value from it.
fn removed_jev_warning(section: &toml::Value, text: &str) -> String {
    let line = text
        .lines()
        .position(|l| {
            let l = l.trim_start();
            l.starts_with("[jev]") || l.starts_with("[jev.")
        })
        .map(|i| i + 1)
        .or_else(|| key_line(text, "jev"))
        .map_or(String::new(), |l| format!(" (line {l})"));
    let mut warning = format!(
        "config.toml{line}: JEV was removed; this section is ignored. Delete it; `swarmux doctor` says whether a key you stored for it is still in the OS keychain. Tool calls no rule matches are settled by you or by guard.unmatched, never by a model"
    );
    if section.get("api_key").is_some() {
        warning.push_str(". Take the api_key line out of this file");
    }
    warning
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
    fn an_old_jev_section_is_ignored_with_a_warning_and_the_rest_still_loads() {
        let s = parse(
            "max_concurrent = 2
[jev]
provider = \"openrouter\"
threshold = 0.9
timeout_ms = 3000
max_allow = \"R2\"
api_key = \"sk-SECRET-123\"
[guard]
unmatched = \"pass\"
",
        )
        .unwrap();
        assert_eq!(s.max_concurrent, 2, "the rest of the file is read");
        assert_eq!(s.guard.unmatched, Unmatched::Pass);
        assert_eq!(s.warnings.len(), 1, "{:#?}", s.warnings);
        let w = &s.warnings[0];
        assert!(
            w.contains("JEV was removed; this section is ignored"),
            "{w}"
        );
        assert!(w.contains("line 2"), "{w}");
        assert!(w.contains("swarmux doctor"), "{w}");
        assert!(w.contains("api_key"), "{w}");
        assert!(!w.contains("SECRET"), "the key is never echoed: {w}");
    }

    #[test]
    fn a_jev_section_of_any_shape_never_breaks_loading() {
        for text in [
            "[jev]\n",
            "[jev]\nprovider = \"gpt\"\nthreshold = 1.5\ntimeout_ms = 0\nmodel = \"x\"\n",
            "jev = { provider = \"typesafe\" }\n",
            "jev = \"typesafe\"\n",
        ] {
            let s = parse(text).unwrap_or_else(|e| panic!("{text:?}: {e}"));
            assert_eq!(s.warnings.len(), 1, "{text:?}: {:?}", s.warnings);
            assert!(s.warnings[0].contains("JEV was removed"), "{text:?}");
        }
        // Without one there is nothing to say.
        assert!(parse("").unwrap().warnings.is_empty());
        // Other unknown keys are still refused.
        assert!(parse("[jevv]\nprovider = \"x\"\n").is_err());
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
    fn the_console_window_is_a_day_unless_the_user_says_otherwise() {
        assert_eq!(parse("").unwrap().console.metrics_hours, 24);
        assert_eq!(
            parse("[console]\nmetrics_hours = 8\n")
                .unwrap()
                .console
                .metrics_hours,
            8
        );
        assert_eq!(
            parse("[console]\nmetrics_hours = 720\n")
                .unwrap()
                .console
                .metrics_hours,
            720
        );
        for bad in [
            "[console]\nmetrics_hours = 0\n",
            "[console]\nmetrics_hours = -3\n",
            "[console]\nmetrics_hours = 721\n",
            "[console]\nmetrics_hours = \"day\"\n",
            "[console]\nmetrics_hours = 1.5\n",
            "[console]\nunknown = 1\n",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        let e = parse("[console]\nmetrics_hours = 0\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("metrics_hours") && e.contains("720"), "{e}");
    }

    #[test]
    fn suggested_send_back_notes_are_the_users_to_set() {
        let default = parse("").unwrap().console.send_back_notes;
        assert_eq!(default.len(), 3, "a few starting points ship");
        let own = parse(
            "[console]\nsend_back_notes = [\" use the staging bucket \", \"run the linter\"]\n",
        )
        .unwrap()
        .console
        .send_back_notes;
        assert_eq!(own, ["use the staging bucket", "run the linter"]);
        // An empty list turns the suggestions off.
        assert!(
            parse("[console]\nsend_back_notes = []\n")
                .unwrap()
                .console
                .send_back_notes
                .is_empty()
        );
        let long = "x".repeat(401);
        let bad = [
            "[console]\nsend_back_notes = [\"\"]\n".to_string(),
            "[console]\nsend_back_notes = [\"  \"]\n".to_string(),
            "[console]\nsend_back_notes = [1]\n".to_string(),
            format!("[console]\nsend_back_notes = [\"{long}\"]\n"),
            format!(
                "[console]\nsend_back_notes = [{}]\n",
                ["\"a\""; 10].join(", ")
            ),
        ];
        for text in bad {
            assert!(parse(&text).is_err(), "{text}");
        }
    }

    #[test]
    fn landing_is_auto_unless_the_user_asks_for_manual() {
        use swarmux_application::LandingPolicy;
        assert_eq!(parse("").unwrap().missions.landing, LandingPolicy::Auto);
        assert_eq!(
            parse("[missions]\nplan_first = true\n")
                .unwrap()
                .missions
                .landing,
            LandingPolicy::Auto,
            "other settings leave it alone"
        );
        for (text, want) in [
            ("[missions]\nlanding = \"auto\"\n", LandingPolicy::Auto),
            ("[missions]\nlanding = \"manual\"\n", LandingPolicy::Manual),
            (
                "[missions]\nlanding = \" manual \"\n",
                LandingPolicy::Manual,
            ),
        ] {
            assert_eq!(parse(text).unwrap().missions.landing, want, "{text}");
        }
        // A bad word is refused with the line and the two it may be.
        for bad in [
            "[missions]\nlanding = \"sometimes\"\n",
            "[missions]\nlanding = \"\"\n",
            "[missions]\nlanding = \"Manual\"\n",
        ] {
            let e = parse(bad).unwrap_err();
            assert!(
                e.message.contains("\"auto\"") && e.message.contains("\"manual\""),
                "{bad}: {e}"
            );
            assert_eq!(e.line, Some(2), "{bad}");
        }
        assert!(parse("[missions]\nlanding = true\n").is_err());
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
    fn an_old_rung_list_naming_the_removed_judge_rung_still_loads() {
        let s = parse(
            "[watchdog]
rungs = [\"nudge\", \"judge\", \"fresh\", \"inbox\"]
",
        )
        .unwrap();
        assert_eq!(
            s.watchdog.ladder.rungs,
            [
                swarmux_domain::Rung::Nudge,
                swarmux_domain::Rung::Fresh,
                swarmux_domain::Rung::Inbox
            ],
            "the removed rung is skipped, the others kept in order"
        );
        assert_eq!(s.warnings.len(), 1, "{:?}", s.warnings);
        assert!(s.warnings[0].contains("line 2"), "{}", s.warnings[0]);
        assert!(
            s.warnings[0].contains("JEV was removed"),
            "{}",
            s.warnings[0]
        );
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

    #[test]
    fn a_mistake_in_the_deck_section_never_refuses_the_file() {
        let s = parse(
            "[deck]
panels = [\"graph\", \"nope\"]
session_rows = \"x\"
colour = 1
",
        )
        .expect("the file loads");
        assert!(s.deck.present);
        assert_eq!(s.deck.unknown, ["colour"]);
        assert!(parse("").unwrap().deck == swarmux_domain::console::DeckConfig::default());
        let good = parse(
            "[deck]
panels = [\"limits\", \"graph\"]
",
        )
        .unwrap();
        assert_eq!(good.deck.panels.as_deref().map(<[String]>::len), Some(2));
    }

    fn entries(text: &str) -> Vec<(String, String, Vec<String>)> {
        parse(text)
            .unwrap()
            .keys
            .entries
            .into_iter()
            .map(|e| (e.section, e.name, e.keys))
            .collect()
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn keys_are_read_as_written_one_or_a_list() {
        let found = entries(concat!(
            "[keys.global]\n",
            "quit = \"x\"\n",
            "\"look.next_theme\" = [\"t\", \"f2\"]\n",
            "[keys.\"mission.wall\"]\n",
            "\"list.next\" = []\n",
            "[keys.focus.reading]\n",
            "\"focus.top\" = \"g g\"\n",
        ));
        assert!(found.contains(&("global".into(), "quit".into(), strings(&["x"]))));
        assert!(found.contains(&(
            "global".into(),
            "look.next_theme".into(),
            strings(&["t", "f2"])
        )));
        assert!(found.contains(&("mission.wall".into(), "list.next".into(), vec![])));
        // A table inside a table is a dotted context.
        assert!(found.contains(&(
            "focus.reading".into(),
            "focus.top".into(),
            strings(&["g g"])
        )));
    }

    #[test]
    fn no_keys_section_is_no_keys() {
        assert!(parse("").unwrap().keys.is_empty());
        assert!(parse("[keys]").unwrap().keys.is_empty());
    }

    #[test]
    fn a_wrong_kind_of_value_in_keys_does_not_refuse_the_file() {
        let s = parse(concat!(
            "max_concurrent = 2\n",
            "[keys.global]\n",
            "quit = 5\n",
            "ok = [\"a\", 3]\n",
            "fine = \"x\"\n",
        ))
        .unwrap();
        assert_eq!(s.max_concurrent, 2, "the rest of the file still loads");
        assert_eq!(s.keys.entries.len(), 1);
        assert_eq!(s.keys.malformed.len(), 2);
        assert_eq!(s.keys.malformed[0].at, "global.ok");
        assert!(s.keys.malformed[0].found.contains("number 3"));
        let s = parse("keys = \"x\"").unwrap();
        assert_eq!(s.keys.malformed.len(), 1);
    }
}
