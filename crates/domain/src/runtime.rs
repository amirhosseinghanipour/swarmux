//! How an agent's runtime is launched: the profile that decides how much of
//! the user's and the repository's own configuration it loads, the version
//! it reports, and the record kept of both in the ledger.
//!
//! Nothing here knows how a particular agent is started. The arguments, the
//! files and the probes of one agent live in its adapter (for Claude Code,
//! `swarmux_infrastructure::claude_cli::runtime`); this is the part every
//! adapter shares.

use std::fmt;

use crate::ledger::{Payload, hex, sha256};

/// How much of the agent's own configuration a run loads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RuntimeProfile {
    /// The user's and the repository's configuration apply, as when the user
    /// starts the agent by hand.
    Inherit,
    /// Only what swarmux generates and what the user's own configuration
    /// grants: the repository's settings, hooks, servers and environment do
    /// not load.
    Controlled,
}

impl RuntimeProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            RuntimeProfile::Inherit => "inherit",
            RuntimeProfile::Controlled => "controlled",
        }
    }

    pub fn parse(s: &str) -> Option<RuntimeProfile> {
        match s.trim() {
            "inherit" => Some(RuntimeProfile::Inherit),
            "controlled" => Some(RuntimeProfile::Controlled),
            _ => None,
        }
    }
}

impl fmt::Display for RuntimeProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The `profile` setting: a profile, or `auto`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProfileSetting {
    /// Unattended work in a repository nobody trusted is controlled; the rest
    /// inherits.
    #[default]
    Auto,
    Inherit,
    Controlled,
}

impl ProfileSetting {
    pub fn as_str(self) -> &'static str {
        match self {
            ProfileSetting::Auto => "auto",
            ProfileSetting::Inherit => "inherit",
            ProfileSetting::Controlled => "controlled",
        }
    }

    pub fn parse(s: &str) -> Option<ProfileSetting> {
        match s.trim() {
            "auto" => Some(ProfileSetting::Auto),
            "inherit" => Some(ProfileSetting::Inherit),
            "controlled" => Some(ProfileSetting::Controlled),
            _ => None,
        }
    }
}

/// What one run is, as far as its profile goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProfileInput {
    /// The `profile` setting.
    pub setting: ProfileSetting,
    /// A profile chosen for this job or its mission.
    pub chosen: Option<RuntimeProfile>,
    /// Nobody watches the run: a mission's job, a reviewer or a job that
    /// continues one.
    pub unattended: bool,
    /// The run is a reviewer's.
    pub review: bool,
    /// The repository is trusted: by a trust record, or, until those
    /// exist, by the user's own configuration.
    pub trusted: bool,
}

/// The profile of a run and why.
pub fn resolve_profile(input: ProfileInput) -> (RuntimeProfile, &'static str) {
    use RuntimeProfile::{Controlled, Inherit};
    // A reviewer reads what the agent wrote: it never loads it as settings.
    if input.review {
        return (Controlled, "reviewer: always controlled");
    }
    if let Some(profile) = input.chosen {
        return (profile, "chosen for the job or its mission");
    }
    match input.setting {
        ProfileSetting::Inherit => (Inherit, "runtime.profile = inherit"),
        ProfileSetting::Controlled => (Controlled, "runtime.profile = controlled"),
        ProfileSetting::Auto if input.unattended && !input.trusted => {
            (Controlled, "unattended in a repository that is not trusted")
        }
        ProfileSetting::Auto if input.unattended => (Inherit, "unattended in a trusted repository"),
        ProfileSetting::Auto => (Inherit, "attended"),
    }
}

/// SHA-256 of `bytes` as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&sha256(bytes))
}

/// A version such as `2.1.323`, ordered by its numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Version {
        Version {
            major,
            minor,
            patch,
        }
    }

    /// The first `major.minor.patch` in `text`: `2.1.323` and
    /// `2.1.323 (Claude Code)` both read as 2.1.323.
    pub fn parse(text: &str) -> Option<Version> {
        let word = text
            .split_whitespace()
            .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))?;
        let core = word.split(['-', '+']).next()?;
        let mut parts = core.split('.').map(|p| p.parse::<u32>().ok());
        let major = parts.next()??;
        let minor = parts.next()??;
        let patch = parts.next().flatten().unwrap_or(0);
        Some(Version {
            major,
            minor,
            patch,
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Where a version stands against what an adapter was tested with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionStanding {
    /// Below the floor the adapter's guarantees need.
    BelowFloor,
    /// At or above the floor, below the tested range.
    BelowTested,
    Tested,
    /// Newer than anything tested.
    AboveTested,
    /// The version could not be read.
    Unknown,
}

impl VersionStanding {
    pub fn as_str(self) -> &'static str {
        match self {
            VersionStanding::BelowFloor => "below_floor",
            VersionStanding::BelowTested => "below_tested",
            VersionStanding::Tested => "tested",
            VersionStanding::AboveTested => "above_tested",
            VersionStanding::Unknown => "unknown",
        }
    }
}

/// The versions an adapter was tested with, and the oldest one its
/// guarantees need.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VersionRange {
    /// Below this, unattended runs are refused.
    pub floor: Version,
    pub tested_min: Version,
    pub tested_max: Version,
}

impl VersionRange {
    pub fn standing(&self, version: Option<Version>) -> VersionStanding {
        let Some(v) = version else {
            return VersionStanding::Unknown;
        };
        if v < self.floor {
            VersionStanding::BelowFloor
        } else if v < self.tested_min {
            VersionStanding::BelowTested
        } else if v > self.tested_max {
            VersionStanding::AboveTested
        } else {
            VersionStanding::Tested
        }
    }
}

/// What the host's own policy does to a run (Claude Code's managed
/// settings, for that adapter), as the adapter read it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostPolicy {
    /// Where policy was found, one line each.
    pub sources: Vec<String>,
    /// The locks that are on, by the agent's own names.
    pub locks: Vec<String>,
    /// What could not be read, one line each.
    pub unreadable: Vec<String>,
    /// Why a run could not launch as swarmux asks, one reason each: the
    /// Guard's hook would be dropped, or a flag swarmux passes is refused.
    pub blocks: Vec<String>,
}

impl HostPolicy {
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty() && self.locks.is_empty() && self.unreadable.is_empty()
    }

    pub fn payload(&self) -> Payload {
        Payload::new()
            .strs("sources", &self.sources)
            .strs("locks", &self.locks)
            .strs("unreadable", &self.unreadable)
            .strs("blocks", &self.blocks)
    }
}

/// What swarmux decided about how a run is launched, known before it starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeLaunch {
    /// The adapter: `claude-code`.
    pub runtime: String,
    pub profile: RuntimeProfile,
    pub profile_reason: String,
    /// The version the adapter found when it probed, if it did.
    pub version: Option<String>,
    pub standing: VersionStanding,
    /// Which configuration sources the run loads, as passed: `default`
    /// (all), or a list.
    pub sources: String,
    /// Only the servers swarmux passes are used.
    pub strict_mcp: bool,
    /// A failing guard hook blocks the call.
    pub on_failure_block: bool,
    /// SHA-256 (hex) of the settings swarmux generated for the run.
    pub settings_hash: String,
    pub host: HostPolicy,
    /// SHA-256 (hex) of the `[runtime]` settings that decided it.
    pub policy_sha256: String,
    /// The ledger entry of the canary that cleared this run, if one did.
    pub canary_seq: Option<u64>,
}

impl RuntimeLaunch {
    pub fn payload(&self) -> Payload {
        Payload::new()
            .str("runtime", &self.runtime)
            .str("profile", self.profile.as_str())
            .str("profile_reason", &self.profile_reason)
            .opt_str("version", self.version.as_deref())
            .str("version_standing", self.standing.as_str())
            .str("sources", &self.sources)
            .bool("strict_mcp", self.strict_mcp)
            .bool("on_failure_block", self.on_failure_block)
            .str("settings_hash", &self.settings_hash)
            .json("host_policy", &self.host.payload().build())
            .str("policy_sha256", &self.policy_sha256)
            .opt_int("canary_seq", self.canary_seq.map(|s| s as i64))
    }
}

/// What the running agent reported about itself (`system/init` for Claude
/// Code), set against what swarmux asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeObserved {
    pub version: Option<String>,
    pub permission_mode: String,
    pub tools: usize,
    /// `name=status` of each MCP server the run connected or tried.
    pub mcp_servers: Vec<String>,
    pub plugins: usize,
    /// What the agent says it can do (`capabilities`).
    pub capabilities: Vec<String>,
    /// Where what it reported differs from what was asked for.
    pub mismatches: Vec<String>,
}

impl RuntimeObserved {
    pub fn payload(&self) -> Payload {
        Payload::new()
            .str("event", "observed")
            .opt_str("version", self.version.as_deref())
            .str("permission_mode", &self.permission_mode)
            .int("tools", self.tools as i64)
            .strs("mcp_servers", &self.mcp_servers)
            .int("plugins", self.plugins as i64)
            .strs("capabilities", &self.capabilities)
            .strs("mismatches", &self.mismatches)
    }
}

/// How one canary ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckStatus {
    /// The policy held: what had to be stopped was stopped.
    Pass,
    /// It did not hold.
    Fail,
    /// It held, or did not, in a way that is known and not relied on: said,
    /// not counted against the run.
    Note,
    /// The canary could not tell (the agent did not make the call, a
    /// timeout, no network): treated as a failure where a run depends on it.
    Inconclusive,
    /// Not run, and why.
    Skipped,
}

impl CheckStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CheckStatus::Pass => "pass",
            CheckStatus::Fail => "fail",
            CheckStatus::Note => "note",
            CheckStatus::Inconclusive => "inconclusive",
            CheckStatus::Skipped => "skipped",
        }
    }
}

/// One canary and what it found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanaryCheck {
    /// `deny`, `hook_exit_1`, `hook_timeout`, `hook_invalid_json`,
    /// `unknown_tool`, `subagent`, `host_policy`, `version`.
    pub name: String,
    pub status: CheckStatus,
    /// What was seen, in a line.
    pub detail: String,
    /// Whether an unattended run may start without this one passing.
    pub required: bool,
}

/// The canaries of one run of the set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CanaryReport {
    pub version: Option<String>,
    pub profile: Option<RuntimeProfile>,
    /// SHA-256 (hex) of the `[runtime]` settings that decided what to do
    /// with the result.
    pub policy_sha256: String,
    pub checks: Vec<CanaryCheck>,
}

impl CanaryReport {
    /// The required checks that did not pass (a `Note` does not count).
    pub fn blocking(&self) -> Vec<&CanaryCheck> {
        self.checks
            .iter()
            .filter(|c| {
                c.required
                    && !matches!(
                        c.status,
                        CheckStatus::Pass | CheckStatus::Note | CheckStatus::Skipped
                    )
            })
            .collect()
    }

    pub fn passed(&self) -> bool {
        self.blocking().is_empty()
    }

    /// The reasons a run is refused, in a line; empty when it passed.
    pub fn refusal(&self) -> String {
        self.blocking()
            .iter()
            .map(|c| format!("{} {}: {}", c.name, c.status.as_str(), c.detail))
            .collect::<Vec<_>>()
            .join("; ")
    }

    pub fn payload(&self, event: &str) -> Payload {
        let checks = self
            .checks
            .iter()
            .map(|c| {
                Payload::new()
                    .str("name", &c.name)
                    .str("status", c.status.as_str())
                    .bool("required", c.required)
                    .text("detail", &c.detail)
            })
            .collect();
        Payload::new()
            .str("event", event)
            .opt_str("version", self.version.as_deref())
            .opt_str("profile", self.profile.map(RuntimeProfile::as_str))
            .bool("passed", self.passed())
            .str("policy_sha256", &self.policy_sha256)
            .objects("checks", checks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> ProfileInput {
        ProfileInput {
            setting: ProfileSetting::Auto,
            chosen: None,
            unattended: false,
            review: false,
            trusted: false,
        }
    }

    #[test]
    fn a_reviewer_is_always_controlled() {
        for setting in [
            ProfileSetting::Auto,
            ProfileSetting::Inherit,
            ProfileSetting::Controlled,
        ] {
            let i = ProfileInput {
                setting,
                chosen: Some(RuntimeProfile::Inherit),
                review: true,
                trusted: true,
                ..input()
            };
            assert_eq!(
                resolve_profile(i).0,
                RuntimeProfile::Controlled,
                "{setting:?}"
            );
        }
    }

    #[test]
    fn auto_controls_unattended_work_in_untrusted_repositories_only() {
        let p = |unattended, trusted| {
            resolve_profile(ProfileInput {
                unattended,
                trusted,
                ..input()
            })
            .0
        };
        assert_eq!(p(false, false), RuntimeProfile::Inherit);
        assert_eq!(p(false, true), RuntimeProfile::Inherit);
        assert_eq!(p(true, false), RuntimeProfile::Controlled);
        assert_eq!(p(true, true), RuntimeProfile::Inherit);
    }

    #[test]
    fn a_choice_for_the_job_beats_the_setting() {
        let i = ProfileInput {
            setting: ProfileSetting::Controlled,
            chosen: Some(RuntimeProfile::Inherit),
            ..input()
        };
        assert_eq!(resolve_profile(i).0, RuntimeProfile::Inherit);
        let i = ProfileInput {
            setting: ProfileSetting::Inherit,
            unattended: true,
            ..input()
        };
        assert_eq!(resolve_profile(i).0, RuntimeProfile::Inherit);
    }

    #[test]
    fn profile_names_round_trip() {
        for p in [RuntimeProfile::Inherit, RuntimeProfile::Controlled] {
            assert_eq!(RuntimeProfile::parse(p.as_str()), Some(p));
        }
        assert_eq!(ProfileSetting::parse("auto"), Some(ProfileSetting::Auto));
        assert_eq!(RuntimeProfile::parse("auto"), None);
        assert_eq!(ProfileSetting::parse("strict"), None);
    }

    #[test]
    fn sha256_hex_is_the_usual_digest() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn versions_parse_and_order() {
        assert_eq!(Version::parse("2.1.323"), Some(Version::new(2, 1, 323)));
        assert_eq!(
            Version::parse("2.1.323 (Claude Code)"),
            Some(Version::new(2, 1, 323))
        );
        assert_eq!(
            Version::parse("claude 2.1.9-beta.1"),
            Some(Version::new(2, 1, 9))
        );
        assert_eq!(Version::parse("2.1"), Some(Version::new(2, 1, 0)));
        assert_eq!(Version::parse("none"), None);
        assert_eq!(Version::parse(""), None);
        assert!(Version::new(2, 1, 295) < Version::new(2, 1, 323));
        assert!(Version::new(2, 1, 1000) < Version::new(2, 2, 0));
        assert_eq!(Version::new(2, 1, 295).to_string(), "2.1.295");
    }

    #[test]
    fn a_version_stands_against_the_range() {
        let r = VersionRange {
            floor: Version::new(2, 1, 295),
            tested_min: Version::new(2, 1, 300),
            tested_max: Version::new(2, 1, 323),
        };
        let at = |s| r.standing(Version::parse(s));
        assert_eq!(at("2.1.294"), VersionStanding::BelowFloor);
        assert_eq!(at("2.1.295"), VersionStanding::BelowTested);
        assert_eq!(at("2.1.300"), VersionStanding::Tested);
        assert_eq!(at("2.1.323"), VersionStanding::Tested);
        assert_eq!(at("2.1.324"), VersionStanding::AboveTested);
        assert_eq!(at("garbage"), VersionStanding::Unknown);
    }

    #[test]
    fn the_launch_record_is_one_json_object() {
        let launch = RuntimeLaunch {
            runtime: "claude-code".into(),
            profile: RuntimeProfile::Controlled,
            profile_reason: "reviewer: always controlled".into(),
            version: Some("2.1.323".into()),
            standing: VersionStanding::Tested,
            sources: "user".into(),
            strict_mcp: true,
            on_failure_block: true,
            settings_hash: "ab".into(),
            policy_sha256: "cd".into(),
            host: HostPolicy {
                locks: vec!["allowManagedHooksOnly".into()],
                blocks: vec!["allowManagedHooksOnly".into()],
                ..HostPolicy::default()
            },
            canary_seq: Some(7),
        };
        let json = launch.payload().build();
        assert_eq!(
            json,
            r#"{"runtime":"claude-code","profile":"controlled","profile_reason":"reviewer: always controlled","version":"2.1.323","version_standing":"tested","sources":"user","strict_mcp":true,"on_failure_block":true,"settings_hash":"ab","host_policy":{"sources":[],"locks":["allowManagedHooksOnly"],"unreadable":[],"blocks":["allowManagedHooksOnly"]},"policy_sha256":"cd","canary_seq":7}"#
        );
    }

    fn check(name: &str, status: CheckStatus, required: bool) -> CanaryCheck {
        CanaryCheck {
            name: name.into(),
            status,
            detail: "d".into(),
            required,
        }
    }

    #[test]
    fn only_required_checks_that_did_not_pass_refuse() {
        let mut report = CanaryReport {
            checks: vec![
                check("deny", CheckStatus::Pass, true),
                check("hook_invalid_json", CheckStatus::Note, false),
                check("subagent", CheckStatus::Inconclusive, false),
            ],
            ..CanaryReport::default()
        };
        assert!(report.passed());
        assert_eq!(report.refusal(), "");
        report
            .checks
            .push(check("hook_exit_1", CheckStatus::Fail, true));
        report
            .checks
            .push(check("hook_timeout", CheckStatus::Inconclusive, true));
        assert!(!report.passed());
        assert_eq!(
            report.refusal(),
            "hook_exit_1 fail: d; hook_timeout inconclusive: d"
        );
        let json = report.payload("canary").build();
        assert!(json.starts_with(r#"{"event":"canary","version":null,"profile":null,"passed":false,"policy_sha256":"","checks":[{"name":"deny","status":"pass","required":true,"detail":"d"}"#));
    }

    #[test]
    fn the_observed_record_names_its_event() {
        let json = RuntimeObserved {
            version: Some("2.1.323".into()),
            permission_mode: "default".into(),
            tools: 3,
            mcp_servers: vec!["board=connected".into()],
            plugins: 0,
            capabilities: vec!["interrupt_receipt_v1".into()],
            mismatches: Vec::new(),
        }
        .payload()
        .build();
        assert!(json.starts_with(r#"{"event":"observed","version":"2.1.323""#));
        assert!(json.contains(r#""mismatches":[]"#));
    }
}
