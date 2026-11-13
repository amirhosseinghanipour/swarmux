use std::path::PathBuf;

use crate::palette::ThemeDef;
use swarmux_domain::alarm::Reversibility;
use swarmux_domain::budget::GovernorPolicy;
use swarmux_domain::telemetry::{MINUTE, PriceTable};
use swarmux_domain::{
    AGENT_CONFIG_PROTECTED, DEFAULT_EXPIRY_DAYS, DEFAULT_MODEL, GuardPolicy, Limits, Pattern,
    PortPlan, RuleScope, Unmatched,
};

/// The `[console]` section: what the Work and Deck screens show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleSettings {
    /// How many hours back the verification figures look (first-run gate
    /// passes, claim to verified, verified an hour), `1..=720`.
    pub metrics_hours: u32,
    /// Notes the send-back line offers to start from (Tab picks one; the
    /// user edits it or sends it as it is). At most [`MAX_SEND_BACK_NOTES`].
    pub send_back_notes: Vec<String>,
}

/// Most suggested notes the send-back line keeps.
pub const MAX_SEND_BACK_NOTES: usize = 9;

impl Default for ConsoleSettings {
    fn default() -> ConsoleSettings {
        ConsoleSettings {
            metrics_hours: crate::metrics::DEFAULT_HOURS,
            send_back_notes: [
                "Read the first failing line of the failed check and fix that before anything else.",
                "Keep the change smaller: touch only what the task needs.",
                "Add or update a test for what you changed, and run it before you say you are done.",
            ]
            .map(String::from)
            .to_vec(),
        }
    }
}

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
    /// Stuck-agent detection and recovery.
    pub watchdog: crate::WatchdogSettings,
    /// Plan-first missions: the default, the planner and the drift line.
    pub missions: crate::PlanSettings,
    /// The Work and Deck screens: what the figures cover.
    pub console: ConsoleSettings,
    /// The Board: typed shared state between the agents of a mission.
    pub board: BoardSettings,
    /// Follow-through: what agents leave behind, standing orders, the brief.
    pub followups: crate::FollowSettings,
    /// The `[keys]` section as written: not yet checked against the
    /// actions that exist, which only the interface knows.
    pub keys: swarmux_domain::keys::KeyConfig,
    /// What was wrong in lines the loader replaced with defaults, one
    /// message each. Shown on start and by `swarmux doctor`.
    pub warnings: Vec<String>,
}

/// The `[board]` section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardSettings {
    /// Mission jobs get the `swarmux-board` MCP server and a digest of the
    /// board when their session starts.
    pub enabled: bool,
}

impl Default for BoardSettings {
    fn default() -> BoardSettings {
        BoardSettings { enabled: true }
    }
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
    /// 4 fps, no tweens or pulses (the same as `motion = "reduced"`).
    pub low_power: bool,
    /// How much the interface moves.
    pub motion: MotionSetting,
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
            motion: MotionSetting::Auto,
            splash: true,
            layouts: Vec::new(),
        }
    }
}

/// The `motion` key: how much the interface moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MotionSetting {
    /// Full motion, dropping to reduced on battery, over SSH or when frames
    /// are slow.
    #[default]
    Auto,
    /// Always full motion.
    Full,
    /// Data updates, spinners and nothing ambient.
    Reduced,
    /// Data updates only.
    Off,
}

impl MotionSetting {
    /// The setting a config value names (case does not matter).
    pub fn parse(value: &str) -> Option<MotionSetting> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(MotionSetting::Auto),
            "full" => Some(MotionSetting::Full),
            "reduced" => Some(MotionSetting::Reduced),
            "off" => Some(MotionSetting::Off),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            MotionSetting::Auto => "auto",
            MotionSetting::Full => "full",
            MotionSetting::Reduced => "reduced",
            MotionSetting::Off => "off",
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

/// What the hook does with a tool call when swarmux can't be reached.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OnUnreachable {
    /// Deny for unattended jobs, leave the call to Claude Code for the rest.
    #[default]
    Auto,
    Deny,
    /// Claude Code asks: in `claude -p` that is a refusal, in a terminal
    /// session the question.
    Ask,
    /// Claude Code's own rules decide, as if there were no hook.
    Pass,
}

/// What a hook prints when it can't reach swarmux.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailMode {
    /// A denial: the call does not run.
    Closed,
    Ask,
    /// Nothing: Claude Code's rules decide.
    Open,
}

impl FailMode {
    /// The value of `SWARMUX_GUARD_FAIL`.
    pub fn as_str(self) -> &'static str {
        match self {
            FailMode::Closed => "closed",
            FailMode::Ask => "ask",
            FailMode::Open => "open",
        }
    }

    pub fn parse(s: &str) -> Option<FailMode> {
        match s.trim() {
            "closed" => Some(FailMode::Closed),
            "ask" => Some(FailMode::Ask),
            "open" => Some(FailMode::Open),
            _ => None,
        }
    }
}

impl OnUnreachable {
    pub fn parse(s: &str) -> Option<OnUnreachable> {
        match s.trim() {
            "auto" => Some(OnUnreachable::Auto),
            "deny" => Some(OnUnreachable::Deny),
            "ask" => Some(OnUnreachable::Ask),
            "pass" => Some(OnUnreachable::Pass),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            OnUnreachable::Auto => "auto",
            OnUnreachable::Deny => "deny",
            OnUnreachable::Ask => "ask",
            OnUnreachable::Pass => "pass",
        }
    }

    /// What a job's hook does: `unattended` jobs (missions, reviewers and
    /// the jobs that continue them) fail closed unless told otherwise.
    pub fn mode(self, unattended: bool) -> FailMode {
        match (self, unattended) {
            (OnUnreachable::Deny, _) | (OnUnreachable::Auto, true) => FailMode::Closed,
            (OnUnreachable::Ask, _) => FailMode::Ask,
            (OnUnreachable::Pass, _) | (OnUnreachable::Auto, false) => FailMode::Open,
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
    /// What a hook does when swarmux can't be reached.
    pub on_unreachable: OnUnreachable,
    /// Where an "always allow" applies.
    pub always_scope: RuleScope,
    /// Days an "always allow" lasts; `None` is never.
    pub always_expiry_days: Option<u32>,
    /// Files whose writes are held (denied for unattended jobs) besides the
    /// built-in ones, as globs from the repository root.
    pub protected_paths: Vec<String>,
    /// Add `onFailure: "block"` to the PreToolUse hook. Claude Code
    /// versions that don't know the key ignore it; detecting the version
    /// is not done yet, so it is off unless asked for.
    pub hook_on_failure_block: bool,
}

impl GuardSettings {
    /// The built-in protected paths and the owner's.
    pub fn protected_globs(&self) -> Vec<String> {
        let mut globs: Vec<String> = AGENT_CONFIG_PROTECTED
            .iter()
            .map(|g| g.to_string())
            .collect();
        for extra in &self.protected_paths {
            if !globs.contains(extra) {
                globs.push(extra.clone());
            }
        }
        globs
    }
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
            watchdog: crate::WatchdogSettings::default(),
            notify: crate::NotifySettings::default(),
            missions: crate::PlanSettings::default(),
            console: ConsoleSettings::default(),
            board: BoardSettings::default(),
            followups: crate::FollowSettings::default(),
            keys: swarmux_domain::keys::KeyConfig::default(),
            warnings: Vec::new(),
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
    /// JEV may allow a call up to this reversibility; above it, and for
    /// every call a person must confirm by typing, it may only deny or
    /// hand the call to the owner. `R1` (changes that stay in the
    /// worktree) by default; `R4` is not a choice.
    pub max_allow: Reversibility,
}

impl Default for JevSettings {
    fn default() -> JevSettings {
        JevSettings {
            provider: JevProvider::Off,
            threshold: 0.85,
            timeout_ms: None,
            max_allow: Reversibility::R1,
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
            on_unreachable: OnUnreachable::Auto,
            always_scope: RuleScope::Repo,
            always_expiry_days: Some(DEFAULT_EXPIRY_DAYS),
            protected_paths: Vec::new(),
            hook_on_failure_block: false,
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
