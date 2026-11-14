//! The agent's runtime: how a run is launched (its profile), what the host
//! allows it, and the checks that must pass before a run nobody watches
//! starts.
//!
//! This is the part every agent adapter shares: the `[runtime]` settings,
//! the seam repository trust fills in, and [`RuntimeGate`], the port an
//! adapter implements. The arguments, probes and canaries of one agent
//! (Claude Code's are in `swarmux_infrastructure::claude_cli::runtime`) stay
//! behind it.

use std::path::{Path, PathBuf};

use swarmux_domain::JobId;
use swarmux_domain::runtime::{
    CanaryReport, ProfileSetting, RuntimeLaunch, RuntimeObserved, RuntimeProfile,
};
use swarmux_domain::telemetry::Environment;

/// When the deny canary runs before an unattended run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CanaryMode {
    /// Before every unattended run's first start (a mission start).
    #[default]
    Each,
    /// Once, then again after `canary_cache_minutes`, or when the agent's
    /// version, the profile or the host's policy changed.
    Cached,
    /// Never. The user's choice, said in `swarmux doctor` and in the ledger.
    Off,
}

impl CanaryMode {
    pub fn as_str(self) -> &'static str {
        match self {
            CanaryMode::Each => "each",
            CanaryMode::Cached => "cached",
            CanaryMode::Off => "off",
        }
    }

    pub fn parse(s: &str) -> Option<CanaryMode> {
        match s.trim() {
            "each" => Some(CanaryMode::Each),
            "cached" => Some(CanaryMode::Cached),
            "off" => Some(CanaryMode::Off),
            _ => None,
        }
    }
}

/// What an unattended run does when a check cannot be made, or fails.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OnError {
    /// The run is refused.
    #[default]
    Refuse,
    /// The run starts, and the ledger and `swarmux doctor` say it was not
    /// cleared.
    Warn,
}

impl OnError {
    pub fn as_str(self) -> &'static str {
        match self {
            OnError::Refuse => "refuse",
            OnError::Warn => "warn",
        }
    }

    pub fn parse(s: &str) -> Option<OnError> {
        match s.trim() {
            "refuse" => Some(OnError::Refuse),
            "warn" => Some(OnError::Warn),
            _ => None,
        }
    }
}

/// Whether the guard hook carries `onFailure: "block"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HookFailure {
    /// When the Claude Code version is known to support it.
    #[default]
    Auto,
    /// Always (the old `guard.hook_on_failure_block = true`).
    Block,
    /// Never.
    Off,
}

impl HookFailure {
    pub fn as_str(self) -> &'static str {
        match self {
            HookFailure::Auto => "auto",
            HookFailure::Block => "block",
            HookFailure::Off => "off",
        }
    }

    pub fn parse(s: &str) -> Option<HookFailure> {
        match s.trim() {
            "auto" => Some(HookFailure::Auto),
            "block" => Some(HookFailure::Block),
            "off" => Some(HookFailure::Off),
            _ => None,
        }
    }
}

/// Whether swarmux's status line replaces the user's own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatuslineMode {
    /// Only when the user has none of their own.
    #[default]
    Auto,
    /// Always: swarmux's status line is on.
    On,
    /// Never.
    Off,
}

impl StatuslineMode {
    pub fn as_str(self) -> &'static str {
        match self {
            StatuslineMode::Auto => "auto",
            StatuslineMode::On => "on",
            StatuslineMode::Off => "off",
        }
    }

    pub fn parse(s: &str) -> Option<StatuslineMode> {
        match s.trim() {
            "auto" => Some(StatuslineMode::Auto),
            "on" => Some(StatuslineMode::On),
            "off" => Some(StatuslineMode::Off),
            _ => None,
        }
    }
}

/// An MCP server a controlled run may use, besides the Board's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServerDef {
    pub name: String,
    /// A program to start (stdio), or empty with a `url`.
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// A remote server's address, with `transport` `http` or `sse`.
    pub url: Option<String>,
    pub transport: String,
}

/// The `[runtime]` section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeSettings {
    pub profile: ProfileSetting,
    /// Settings files a controlled run loads (`user`, `project`, `local`).
    pub controlled_sources: Vec<String>,
    /// Settings files a reviewer loads.
    pub reviewer_sources: Vec<String>,
    /// Repositories the user trusts, until trust records exist.
    pub trusted_repos: Vec<PathBuf>,
    /// The servers a controlled run may use besides the Board's.
    pub mcp: Vec<McpServerDef>,
    pub canary: CanaryMode,
    pub canary_cache_minutes: u64,
    pub canary_timeout_secs: u64,
    /// What an unattended run does when a check fails or cannot be made.
    pub on_error: OnError,
    pub hook_failure: HookFailure,
    pub statusline: StatuslineMode,
}

impl Default for RuntimeSettings {
    fn default() -> RuntimeSettings {
        RuntimeSettings {
            profile: ProfileSetting::Auto,
            controlled_sources: vec!["user".into()],
            reviewer_sources: Vec::new(),
            trusted_repos: Vec::new(),
            mcp: Vec::new(),
            canary: CanaryMode::Each,
            canary_cache_minutes: 60,
            canary_timeout_secs: 90,
            on_error: OnError::Refuse,
            hook_failure: HookFailure::Auto,
            statusline: StatuslineMode::Auto,
        }
    }
}

/// Whether a repository's own agent configuration may load.
///
/// The seam repository trust (X4) fills in: until a trust record store
/// exists, [`ConfiguredTrust`] reads the user's own list.
pub trait RepoTrust {
    fn trusted(&self, repo: &Path) -> bool;
}

/// The interim trust: the `[runtime.repos."<path>"] trusted = true` entries
/// of the user's configuration.
pub struct ConfiguredTrust {
    repos: Vec<PathBuf>,
}

impl ConfiguredTrust {
    pub fn new(repos: Vec<PathBuf>) -> ConfiguredTrust {
        ConfiguredTrust { repos }
    }
}

impl RepoTrust for ConfiguredTrust {
    /// A repository is trusted when it is a listed folder or inside one.
    fn trusted(&self, repo: &Path) -> bool {
        self.repos.iter().any(|t| repo.starts_with(t))
    }
}

/// What the adapter needs to know about a run to describe how it launches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LaunchIntent {
    pub job: JobId,
    pub profile: RuntimeProfile,
    pub review: bool,
    pub board: bool,
    pub unattended: bool,
    /// The session loads none of the user's own settings.
    pub lean: bool,
}

/// Whether a run nobody watches may start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Clearance {
    /// The check is running; ask again later.
    Pending,
    /// May start. A report is carried the first time after the canary ran:
    /// the caller records it.
    Cleared(Option<CanaryReport>),
    /// May not start.
    Refused { why: String, report: CanaryReport },
}

/// What an agent adapter offers the services that start runs.
///
/// Every method answers at once: a check that takes time (a canary calls
/// the agent) runs in the background and shows as [`Clearance::Pending`].
pub trait RuntimeGate {
    /// Whether an unattended run may start now (see [`CanaryMode`]).
    fn clearance(&self, profile: RuntimeProfile) -> Clearance;
    /// How the run launches: profile, version, generated settings.
    fn launch(&self, intent: &LaunchIntent) -> Option<RuntimeLaunch>;
    /// What the agent reported about itself, set against `launch`.
    fn observe(&self, launch: Option<&RuntimeLaunch>, env: &Environment) -> RuntimeObserved;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_fail_closed() {
        let s = RuntimeSettings::default();
        assert_eq!(s.canary, CanaryMode::Each);
        assert_eq!(s.on_error, OnError::Refuse);
        assert_eq!(s.profile, ProfileSetting::Auto);
        assert_eq!(s.controlled_sources, ["user"]);
        assert!(s.reviewer_sources.is_empty());
    }

    #[test]
    fn a_trusted_folder_trusts_what_is_inside_it() {
        let t = ConfiguredTrust::new(vec![PathBuf::from("/w/app")]);
        assert!(t.trusted(Path::new("/w/app")));
        assert!(t.trusted(Path::new("/w/app/sub")));
        assert!(!t.trusted(Path::new("/w/application")));
        assert!(!t.trusted(Path::new("/w")));
    }

    #[test]
    fn names_round_trip() {
        for m in [CanaryMode::Each, CanaryMode::Cached, CanaryMode::Off] {
            assert_eq!(CanaryMode::parse(m.as_str()), Some(m));
        }
        for m in [HookFailure::Auto, HookFailure::Block, HookFailure::Off] {
            assert_eq!(HookFailure::parse(m.as_str()), Some(m));
        }
        for m in [
            StatuslineMode::Auto,
            StatuslineMode::On,
            StatuslineMode::Off,
        ] {
            assert_eq!(StatuslineMode::parse(m.as_str()), Some(m));
        }
        for m in [OnError::Refuse, OnError::Warn] {
            assert_eq!(OnError::parse(m.as_str()), Some(m));
        }
        assert_eq!(CanaryMode::parse("sometimes"), None);
    }
}
