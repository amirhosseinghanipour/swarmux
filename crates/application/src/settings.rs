use std::path::PathBuf;

use crate::palette::ThemeDef;
use swarmux_domain::budget::GovernorPolicy;
use swarmux_domain::telemetry::{MINUTE, PriceTable};
use swarmux_domain::{DEFAULT_MODEL, GuardPolicy, Limits, Pattern, PortPlan, Unmatched};

/// User settings (`config.toml`); every field has a default.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Jobs that may run at the same time.
    pub max_concurrent: usize,
    /// Queued jobs wait while free memory is below this.
    pub min_free_ram_mb: u64,
    /// Model for new sessions.
    pub default_model: String,
    pub guard: GuardSettings,
    pub jev: JevSettings,
    pub review: ReviewSettings,
    pub telemetry: TelemetrySettings,
    /// Built-in model prices with the `[pricing]` overrides applied.
    pub pricing: PriceTable,
    /// Look and feel: theme, layout, icons, animation.
    pub ui: UiSettings,
    pub worktree: WorktreeSettings,
    pub governor: GovernorSettings,
    pub notify: crate::NotifySettings,
}

/// The `[governor]` section: when queued work waits for a usage window to
/// reset, runs on a cheaper model, or runs fewer at once.
#[derive(Clone, Debug, PartialEq)]
pub struct GovernorSettings {
    /// Off: every queued job is admitted as before.
    pub enabled: bool,
    /// New jobs wait while a usage window is at least this full (0 to 1),
    /// and start by themselves once it resets.
    pub hold_at: f64,
    /// Jobs on a pricier model than `downgrade_to` run on it from here (0 to 1).
    pub downgrade_at: f64,
    pub downgrade_to: String,
    /// How far ahead a job's window use is forecast.
    pub horizon_minutes: u32,
    /// Jobs that may run at once on battery.
    pub battery_concurrency: usize,
    /// Dollar cap of every job (`--max-budget-usd`); missions use their
    /// own reservation.
    pub max_job_usd: Option<f64>,
    /// Tell each job its budget and the windows in a line after the prompt.
    pub budget_line: bool,
    /// New jobs wait while the user's inbox load is above this share
    /// (0 to 1); `1.0` turns the hold off.
    pub load_hold_at: f64,
}

impl Default for GovernorSettings {
    fn default() -> GovernorSettings {
        let policy = GovernorPolicy::default();
        GovernorSettings {
            enabled: true,
            hold_at: policy.hold_at,
            downgrade_at: policy.downgrade_at,
            downgrade_to: policy.downgrade_to,
            horizon_minutes: (policy.horizon / MINUTE) as u32,
            battery_concurrency: 2,
            max_job_usd: None,
            budget_line: true,
            load_hold_at: policy.load_hold_at,
        }
    }
}

impl GovernorSettings {
    pub fn policy(&self) -> GovernorPolicy {
        GovernorPolicy {
            hold_at: self.hold_at,
            downgrade_at: self.downgrade_at,
            downgrade_to: self.downgrade_to.clone(),
            horizon: i64::from(self.horizon_minutes) * MINUTE,
            load_hold_at: self.load_hold_at,
        }
    }
}

/// The `[worktree]` section: where job worktrees live and what they get.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeSettings {
    /// Where worktrees are created; `None` means `~/.swarmux/worktrees`.
    pub root: Option<PathBuf>,
    /// Files and folders (relative to the repository) copied into every
    /// new worktree when they exist, such as `.env` and other ignored files.
    pub copy: Vec<String>,
    /// How port blocks are handed out to jobs.
    pub ports: PortPlan,
    /// Worktrees of jobs that ended this long ago are reclaimed.
    pub stale_hours: u64,
}

impl Default for WorktreeSettings {
    fn default() -> WorktreeSettings {
        WorktreeSettings {
            root: None,
            copy: vec![
                ".env".into(),
                ".env.local".into(),
                ".claude/settings.local.json".into(),
            ],
            ports: PortPlan::default(),
            stale_hours: 12,
        }
    }
}

/// The `[telemetry]` section: which background sources run and how often.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TelemetrySettings {
    /// Spend and sessions from Claude Code transcripts.
    pub usage: bool,
    /// CPU, memory, network, disk, battery and processes.
    pub system: bool,
    pub git: bool,
    /// Pull requests and CI through the `gh` CLI.
    pub github: bool,
    /// Other Claude Code sessions, from `claude agents --json`.
    pub agents: bool,
    pub usage_secs: u64,
    pub system_secs: u64,
    pub git_secs: u64,
    pub github_secs: u64,
    pub agents_secs: u64,
    /// Where transcripts are read from; `None` means `~/.claude/projects`
    /// (or `$CLAUDE_CONFIG_DIR/projects`).
    pub transcripts_path: Option<PathBuf>,
}

impl Default for TelemetrySettings {
    fn default() -> TelemetrySettings {
        TelemetrySettings {
            usage: true,
            system: true,
            git: true,
            github: true,
            agents: true,
            usage_secs: 15,
            system_secs: 1,
            git_secs: 5,
            github_secs: 60,
            agents_secs: 15,
            transcripts_path: None,
        }
    }
}

/// Look and feel, from the top-level `theme`, `layout`, `icons`, `gaps`,
/// `fps`, `low_power` and `splash` keys and `[[layouts]]`. Names are kept
/// as written; the interface resolves them and says when one is unknown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiSettings {
    pub theme: String,
    pub layout: String,
    /// `"unicode"` (the default), `"ascii"`, `"nerd"` (Nerd Font glyphs) or
    /// `"auto"` (Nerd when the environment or `font` says so).
    pub icons: String,
    /// The terminal font, as a hint for `icons = "auto"`.
    pub font: String,
    /// `bg = "none"`: leave the background to the terminal.
    pub transparent: bool,
    /// Themes defined in `[themes.<name>]`.
    pub themes: Vec<ThemeDef>,
    /// Blank cells between panels (0-4).
    pub gaps: u16,
    /// Frames per second (1-30).
    pub fps: u16,
    /// 4 fps, no tweens or pulses.
    pub low_power: bool,
    /// The start-up screen.
    pub splash: bool,
    /// User-defined layouts.
    pub layouts: Vec<LayoutSettings>,
}

impl Default for UiSettings {
    fn default() -> UiSettings {
        UiSettings {
            theme: "reference".into(),
            layout: "command".into(),
            icons: "unicode".into(),
            font: String::new(),
            transparent: false,
            themes: Vec::new(),
            gaps: 1,
            fps: 10,
            low_power: false,
            splash: true,
            layouts: Vec::new(),
        }
    }
}

/// One `[[layouts]]` entry: rows of panels, top to bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutSettings {
    pub name: String,
    pub rows: Vec<RowSettings>,
}

/// A row of a user layout. Panels are ids, optionally with a weight
/// (`"timeline:2"`). A row is `height` rows tall, or shares the free
/// space by `weight` (default 1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowSettings {
    pub panels: Vec<String>,
    pub height: Option<u16>,
    pub weight: Option<u16>,
}

/// The `[review]` section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewSettings {
    /// Review new jobs unless turned off (`v` on the dashboard toggles it).
    pub enabled: bool,
    /// Model of the reviewer.
    pub model: String,
    /// Which finished jobs the reviewer reads.
    pub when: ReviewWhen,
}

/// Which finished jobs get a review, when reviews are on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReviewWhen {
    /// Only changes whose risk is elevated or worse: big ones, deletions,
    /// builds, dependencies, pipelines, secrets, changed tests.
    #[default]
    Risk,
    /// Every one.
    Always,
}

impl ReviewWhen {
    pub fn as_str(self) -> &'static str {
        match self {
            ReviewWhen::Risk => "risk",
            ReviewWhen::Always => "always",
        }
    }

    pub fn parse(s: &str) -> Option<ReviewWhen> {
        match s.trim() {
            "risk" => Some(ReviewWhen::Risk),
            "always" => Some(ReviewWhen::Always),
            _ => None,
        }
    }
}

impl Default for ReviewSettings {
    fn default() -> ReviewSettings {
        ReviewSettings {
            enabled: false,
            model: "opus".to_string(),
            when: ReviewWhen::Risk,
        }
    }
}

/// The `[guard]` section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuardSettings {
    pub allow: Vec<Pattern>,
    pub deny: Vec<Pattern>,
    /// What happens to calls no rule matches.
    pub unmatched: Unmatched,
    /// Unanswered approvals are denied after this long.
    pub approval_timeout_secs: u64,
    /// Identical failures in a row that stop a job (0 = never).
    pub loop_repeats: u32,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            max_concurrent: 3,
            min_free_ram_mb: 1536,
            default_model: DEFAULT_MODEL.to_string(),
            guard: GuardSettings::default(),
            jev: JevSettings::default(),
            review: ReviewSettings::default(),
            telemetry: TelemetrySettings::default(),
            pricing: PriceTable::builtin(),
            ui: UiSettings::default(),
            worktree: WorktreeSettings::default(),
            governor: GovernorSettings::default(),
            notify: crate::NotifySettings::default(),
        }
    }
}

/// Which hosted JEV provider to ask, if any.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JevProvider {
    #[default]
    Off,
    TypeSafe,
    OpenRouter,
}

impl JevProvider {
    pub const ALL: [JevProvider; 3] = [
        JevProvider::Off,
        JevProvider::TypeSafe,
        JevProvider::OpenRouter,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            JevProvider::Off => "off",
            JevProvider::TypeSafe => "typesafe",
            JevProvider::OpenRouter => "openrouter",
        }
    }

    pub fn parse(s: &str) -> Option<JevProvider> {
        JevProvider::ALL.into_iter().find(|p| p.as_str() == s)
    }

    /// How long a decision may take before swarmux stops waiting.
    pub fn default_timeout_ms(self) -> u64 {
        match self {
            JevProvider::OpenRouter => 5_000,
            JevProvider::Off | JevProvider::TypeSafe => 2_000,
        }
    }
}

/// The `[jev]` section: the optional hosted decision model.
#[derive(Clone, Debug, PartialEq)]
pub struct JevSettings {
    pub provider: JevProvider,
    /// Answers below this confidence go to the user (or to Haiku) instead.
    pub threshold: f64,
    /// `None` uses the provider's default.
    pub timeout_ms: Option<u64>,
}

impl Default for JevSettings {
    fn default() -> JevSettings {
        JevSettings {
            provider: JevProvider::Off,
            threshold: 0.85,
            timeout_ms: None,
        }
    }
}

impl JevSettings {
    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
            .unwrap_or_else(|| self.provider.default_timeout_ms())
    }
}

impl Default for GuardSettings {
    fn default() -> GuardSettings {
        GuardSettings {
            allow: Vec::new(),
            deny: Vec::new(),
            unmatched: Unmatched::Ask,
            approval_timeout_secs: 120,
            loop_repeats: 2,
        }
    }
}

impl Settings {
    pub fn limits(&self) -> Limits {
        Limits {
            max_concurrent: self.max_concurrent,
            min_free_ram: self.min_free_ram_mb * 1024 * 1024,
        }
    }
}

impl GuardSettings {
    /// The configured rules plus the built-in ones.
    pub fn policy(&self) -> GuardPolicy {
        GuardPolicy::new(self.allow.clone(), self.deny.clone(), self.unmatched)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_limits() {
        let s = Settings::default();
        assert_eq!((s.max_concurrent, s.default_model.as_str()), (3, "sonnet"));
        assert_eq!(s.limits().min_free_ram, 1536 * 1024 * 1024);
        assert_eq!(s.guard.unmatched, Unmatched::Ask);
        assert_eq!(
            (s.guard.approval_timeout_secs, s.guard.loop_repeats),
            (120, 2)
        );
        assert_eq!((s.review.enabled, s.review.model.as_str()), (false, "opus"));
        let t = &s.telemetry;
        assert!(t.usage && t.system && t.git && t.github);
        assert_eq!(
            (t.system_secs, t.git_secs, t.usage_secs, t.github_secs),
            (1, 5, 15, 60)
        );
        assert_eq!(s.pricing, PriceTable::builtin());
        assert_eq!(
            (s.ui.theme.as_str(), s.ui.layout.as_str()),
            ("reference", "command")
        );
        assert_eq!(
            (s.ui.icons.as_str(), s.ui.gaps, s.ui.fps),
            ("unicode", 1, 10)
        );
        assert!(s.ui.splash && !s.ui.low_power && s.ui.layouts.is_empty());
        let g = &s.governor;
        assert!(g.enabled && g.budget_line);
        assert_eq!(
            (g.hold_at, g.battery_concurrency, g.max_job_usd),
            (0.97, 2, None)
        );
        assert_eq!(g.policy(), GovernorPolicy::default());
    }

    #[test]
    fn jev_is_off_by_default_with_provider_timeouts() {
        let j = JevSettings::default();
        assert_eq!((j.provider, j.threshold), (JevProvider::Off, 0.85));
        let typesafe = JevSettings {
            provider: JevProvider::TypeSafe,
            ..j.clone()
        };
        assert_eq!(typesafe.timeout_ms(), 2_000);
        let openrouter = JevSettings {
            provider: JevProvider::OpenRouter,
            ..j
        };
        assert_eq!(openrouter.timeout_ms(), 5_000);
        for p in JevProvider::ALL {
            assert_eq!(JevProvider::parse(p.as_str()), Some(p));
        }
        assert_eq!(JevProvider::parse("x"), None);
    }

    #[test]
    fn policy_includes_builtin_rules() {
        let mut g = GuardSettings::default();
        g.allow.push(Pattern::parse("Bash(make:*)").unwrap());
        let p = g.policy();
        assert_eq!(p.allow_rules()[0].to_string(), "Bash(make:*)");
        assert!(p.deny_rules().len() >= swarmux_domain::BUILTIN_DENY.len());
    }
}
