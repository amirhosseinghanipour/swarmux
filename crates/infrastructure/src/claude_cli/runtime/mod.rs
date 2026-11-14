//! Everything about launching Claude Code that is specific to Claude Code:
//! the launch profiles as arguments, the version range, the host policy
//! probe, the canaries, and the adapter of the application's `RuntimeGate`.
//!
//! Nothing outside this module and `claude_cli::{command, runner}` knows
//! these things. The rest of swarmux sees a [`ClaudeRuntime`] only as an
//! `Rc<dyn RuntimeGate>` (`swarmux_application::runtime`) and a
//! [`LaunchSpec`] handed to the runner, so that the whole module can become
//! the first agent adapter without anything else changing.

pub mod canary;
pub mod host;
pub mod launch;
pub mod range;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use swarmux_application::runtime::{
    CanaryMode, Clearance, LaunchIntent, OnError, RuntimeGate, RuntimeSettings,
};
use swarmux_domain::runtime::{
    CanaryCheck, CanaryReport, CheckStatus, RuntimeLaunch, RuntimeObserved, RuntimeProfile,
    Version, VersionStanding,
};
use swarmux_domain::telemetry::Environment;

use self::canary::{CanarySetup, Scenario};
use self::launch::{LaunchSpec, Probes, describe_sources};

pub use self::launch::LaunchConfig;

/// The name of this adapter in a record.
pub const RUNTIME_NAME: &str = "claude-code";
/// With `canary = "each"`, jobs that start within this long of a canary
/// that passed share it (a task graph starts several at once).
const EACH_REUSE: Duration = Duration::from_secs(30);

/// Runs one canary; replaceable so the gate can be tried with a fake.
pub type CanaryRunner = dyn Fn(&CanarySetup, Scenario) -> CanaryCheck + Send + Sync;

struct Finished {
    at: Instant,
    /// What it was run for: version, profile and host policy.
    key: String,
    check: CanaryCheck,
    /// The caller has not been given this result yet (it records it once).
    fresh: bool,
}

#[derive(Default)]
struct GateState {
    running: bool,
    last: Option<Finished>,
}

/// Claude Code as the runtime of a run.
pub struct ClaudeRuntime {
    bin: PathBuf,
    /// The swarmux executable the canary hook runs.
    exe: PathBuf,
    settings: RuntimeSettings,
    spec: LaunchSpec,
    probes: Probes,
    state: Arc<Mutex<GateState>>,
    runner: Arc<CanaryRunner>,
}

fn locked(state: &Mutex<GateState>) -> MutexGuard<'_, GateState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ClaudeRuntime {
    pub fn new(
        bin: PathBuf,
        exe: PathBuf,
        settings: RuntimeSettings,
        spec: LaunchSpec,
        probes: Probes,
    ) -> ClaudeRuntime {
        ClaudeRuntime {
            bin,
            exe,
            settings,
            spec,
            probes,
            state: Arc::default(),
            runner: Arc::new(canary::run),
        }
    }

    /// Runs the canaries through `runner` instead of `claude` (tests).
    pub fn with_canary_runner(mut self, runner: Arc<CanaryRunner>) -> ClaudeRuntime {
        self.runner = runner;
        self
    }

    pub fn probes(&self) -> &Probes {
        &self.probes
    }

    pub fn spec(&self) -> &LaunchSpec {
        &self.spec
    }

    /// The version found when swarmux started.
    pub fn version(&self) -> Option<Version> {
        self.probes.version
    }

    fn standing(&self) -> VersionStanding {
        range::range().standing(self.probes.version)
    }

    fn setup(&self, profile: RuntimeProfile) -> CanarySetup {
        let controlled = profile == RuntimeProfile::Controlled;
        CanarySetup {
            bin: self.bin.clone(),
            exe: self.exe.clone(),
            sources: controlled.then(|| self.spec.config.controlled_sources.join(",")),
            strict_mcp: controlled,
            on_failure_block: self.spec.on_failure_block,
            timeout: Duration::from_secs(self.settings.canary_timeout_secs.max(10)),
            env: Vec::new(),
        }
    }

    /// The checks that need no call: the version and the host's policy.
    pub fn static_checks(&self, profile: RuntimeProfile) -> Vec<CanaryCheck> {
        let mut checks = Vec::new();
        let r = range::range();
        let (status, detail) = match self.standing() {
            VersionStanding::Unknown => (
                CheckStatus::Fail,
                "the version of claude could not be read (`claude --version`)".to_string(),
            ),
            VersionStanding::BelowFloor => (
                CheckStatus::Fail,
                format!(
                    "claude {} is older than {}, the first version whose hooks can fail closed (onFailure)",
                    self.version().map_or_else(String::new, |v| v.to_string()),
                    r.floor
                ),
            ),
            VersionStanding::BelowTested | VersionStanding::AboveTested => (
                CheckStatus::Note,
                format!(
                    "claude {} is outside the tested range {}..{}; the canaries decide",
                    self.version().map_or_else(String::new, |v| v.to_string()),
                    r.tested_min,
                    r.tested_max
                ),
            ),
            VersionStanding::Tested => (
                CheckStatus::Pass,
                format!(
                    "claude {} is in the tested range {}..{}",
                    self.version().map_or_else(String::new, |v| v.to_string()),
                    r.tested_min,
                    r.tested_max
                ),
            ),
        };
        checks.push(CanaryCheck {
            name: "version".into(),
            status,
            detail,
            required: true,
        });
        let host = &self.probes.host;
        let (status, detail) = if host.blocks.is_empty() {
            let found = if host.locks.is_empty() {
                "no managed lock found".to_string()
            } else {
                format!(
                    "managed locks that do not affect the launch: {}",
                    host.locks.join(", ")
                )
            };
            (CheckStatus::Pass, found)
        } else {
            (CheckStatus::Fail, host.blocks.join("; "))
        };
        checks.push(CanaryCheck {
            name: "host_policy".into(),
            status,
            detail,
            required: true,
        });
        let _ = profile;
        checks
    }

    fn report(&self, profile: RuntimeProfile, mut checks: Vec<CanaryCheck>) -> CanaryReport {
        checks.sort_by_key(|c| c.name != "version" && c.name != "host_policy");
        CanaryReport {
            version: self.probes.version.map(|v| v.to_string()),
            profile: Some(profile),
            policy_sha256: self.policy_hash(),
            checks,
        }
    }

    /// SHA-256 of the `[runtime]` settings, written out the same way each
    /// time, so a record names the configuration that decided it.
    fn policy_hash(&self) -> String {
        let s = &self.settings;
        let text = format!(
            "profile={}
controlled_sources={}
reviewer_sources={}
canary={}
cache={}
timeout={}
on_error={}
hook_failure={}
statusline={}
trusted={:?}
mcp={:?}
",
            s.profile.as_str(),
            s.controlled_sources.join(","),
            s.reviewer_sources.join(","),
            s.canary.as_str(),
            s.canary_cache_minutes,
            s.canary_timeout_secs,
            s.on_error.as_str(),
            s.hook_failure.as_str(),
            s.statusline.as_str(),
            s.trusted_repos,
            s.mcp,
        );
        swarmux_domain::runtime::sha256_hex(text.as_bytes())
    }

    /// The key a canary result is kept under: a different version, profile
    /// or host policy starts a new one.
    fn key(&self, profile: RuntimeProfile) -> String {
        format!(
            "{}|{profile}|{}|{}",
            self.probes
                .version
                .map_or_else(String::new, |v| v.to_string()),
            self.probes.host.locks.join(","),
            self.probes.host.blocks.len()
        )
    }

    fn conclude(&self, report: CanaryReport, first: bool) -> Clearance {
        if report.passed() {
            return Clearance::Cleared(first.then_some(report));
        }
        match self.settings.on_error {
            OnError::Refuse => Clearance::Refused {
                why: report.refusal(),
                report,
            },
            OnError::Warn => {
                let mut report = report;
                report.checks.push(CanaryCheck {
                    name: "on_error".into(),
                    status: CheckStatus::Note,
                    detail: format!(
                        "runtime.on_error = warn: started although {}",
                        report.refusal()
                    ),
                    required: false,
                });
                Clearance::Cleared(Some(report))
            }
        }
    }

    /// Runs the deny canary on its own thread, once at a time.
    fn start_canary(&self, profile: RuntimeProfile) {
        {
            let mut st = locked(&self.state);
            if st.running {
                return;
            }
            st.running = true;
        }
        let setup = self.setup(profile);
        let runner = self.runner.clone();
        let state = self.state.clone();
        let key = self.key(profile);
        std::thread::spawn(move || {
            let mut check = runner(&setup, Scenario::Deny);
            // The model not making the call is not a verdict: ask once more.
            if check.status == CheckStatus::Inconclusive {
                check = runner(&setup, Scenario::Deny);
            }
            let mut st = locked(&state);
            st.running = false;
            st.last = Some(Finished {
                at: Instant::now(),
                key,
                check,
                fresh: true,
            });
        });
    }

    /// Whether the last canary result may be used for `profile` now.
    fn reusable(&self, last: &Finished, profile: RuntimeProfile) -> bool {
        let age = last.at.elapsed();
        match self.settings.canary {
            CanaryMode::Each => age < EACH_REUSE && last.key == self.key(profile),
            CanaryMode::Cached => {
                age < Duration::from_secs(self.settings.canary_cache_minutes * 60)
                    && last.key == self.key(profile)
            }
            CanaryMode::Off => true,
        }
    }

    /// Every canary, one after the other, for `swarmux doctor --conformance`.
    pub fn conformance(&self, profile: RuntimeProfile) -> CanaryReport {
        let setup = self.setup(profile);
        let mut checks = self.static_checks(profile);
        for scenario in Scenario::ALL {
            checks.push((self.runner)(&setup, scenario));
        }
        self.report(profile, checks)
    }

    /// Runs the canary of one scenario.
    pub fn run_scenario(&self, profile: RuntimeProfile, scenario: Scenario) -> CanaryCheck {
        (self.runner)(&self.setup(profile), scenario)
    }
}

impl RuntimeGate for ClaudeRuntime {
    fn clearance(&self, profile: RuntimeProfile) -> Clearance {
        let statics = self.static_checks(profile);
        let static_failed = statics
            .iter()
            .any(|c| c.required && !matches!(c.status, CheckStatus::Pass | CheckStatus::Note));
        if static_failed {
            // No call is spent on a run that cannot start anyway.
            return self.conclude(self.report(profile, statics), true);
        }
        if self.settings.canary == CanaryMode::Off {
            let mut checks = statics;
            checks.push(CanaryCheck {
                name: "deny".into(),
                status: CheckStatus::Skipped,
                detail: "runtime.canary = off: the deny canary was not run".into(),
                required: true,
            });
            return Clearance::Cleared(Some(self.report(profile, checks)));
        }
        let usable = {
            let mut st = locked(&self.state);
            match st.last.as_mut() {
                Some(last) if self.reusable(last, profile) => {
                    let first = std::mem::take(&mut last.fresh);
                    Some((last.check.clone(), first))
                }
                _ => None,
            }
        };
        match usable {
            Some((check, first)) => {
                let mut checks = statics;
                checks.push(check);
                self.conclude(self.report(profile, checks), first)
            }
            None => {
                self.start_canary(profile);
                Clearance::Pending
            }
        }
    }

    fn launch(&self, intent: &LaunchIntent) -> Option<RuntimeLaunch> {
        let controlled = intent.review || intent.profile == RuntimeProfile::Controlled;
        let cfg = &self.spec.config;
        let sources = if intent.review {
            Some(cfg.reviewer_sources.join(","))
        } else if controlled {
            let kept: Vec<&str> = cfg
                .controlled_sources
                .iter()
                .map(String::as_str)
                .filter(|s| !(intent.lean && *s == "user"))
                .collect();
            Some(kept.join(","))
        } else {
            intent.lean.then(|| "project,local".to_string())
        };
        let settings = self.spec.settings_json(controlled).unwrap_or_default();
        Some(RuntimeLaunch {
            runtime: RUNTIME_NAME.to_string(),
            profile: if intent.review {
                RuntimeProfile::Controlled
            } else {
                intent.profile
            },
            profile_reason: String::new(),
            version: self.probes.version.map(|v| v.to_string()),
            standing: self.standing(),
            sources: describe_sources(sources.as_deref()),
            strict_mcp: controlled,
            on_failure_block: self.spec.on_failure_block && self.spec.guard.is_some(),
            settings_hash: swarmux_domain::runtime::sha256_hex(settings.as_bytes()),
            host: self.probes.host.clone(),
            policy_sha256: self.policy_hash(),
            canary_seq: None,
        })
    }

    fn observe(&self, launch: Option<&RuntimeLaunch>, env: &Environment) -> RuntimeObserved {
        let mut mismatches = Vec::new();
        if let (Some(asked), Some(said)) = (
            self.probes.version,
            env.claude_code_version.as_deref().and_then(Version::parse),
        ) && asked != said
        {
            mismatches.push(format!(
                "claude reports {said}, but {asked} was found at startup"
            ));
        }
        if launch.is_some_and(|l| l.strict_mcp) {
            let board = crate::board::SERVER_NAME;
            for server in &env.mcp_servers {
                let allowed = server.name == board
                    || self.spec.config.mcp.iter().any(|d| d.name == server.name);
                if !allowed {
                    mismatches.push(format!(
                        "MCP server \"{}\" ({}) was loaded although only swarmux's servers were passed",
                        server.name, server.status
                    ));
                }
            }
        }
        RuntimeObserved {
            version: env.claude_code_version.clone(),
            permission_mode: env.permission_mode.clone(),
            tools: env.tools.len(),
            mcp_servers: env
                .mcp_servers
                .iter()
                .map(|s| format!("{}={}", s.name, s.status))
                .collect(),
            plugins: env.plugins.len(),
            capabilities: env.capabilities.clone(),
            mismatches,
        }
    }
}

#[cfg(test)]
mod tests;
