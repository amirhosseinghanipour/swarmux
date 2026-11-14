//! The launch profiles as `claude` arguments.
//!
//! `inherit` is how swarmux has always started `claude`: the user's and
//! the repository's own configuration apply. `controlled` loads the
//! settings files named in `[runtime] controlled_sources` (the user's, by
//! default), passes `--strict-mcp-config` so only the servers swarmux
//! generates are used, and so leaves out the repository's `.claude`
//! settings, `.mcp.json`, `CLAUDE.md` and environment (see the spike in
//! `guide/claude-code.md`: `--setting-sources user` drops the repository's
//! `CLAUDE.md` too). A reviewer is always controlled, with the
//! `reviewer_sources`.

use serde_json::{Map, Value, json};
use swarmux_application::ports::RunRequest;
use swarmux_application::{HookFailure, McpServerDef, RuntimeSettings, StatuslineMode};
use swarmux_domain::runtime::{RuntimeProfile, Version};

use super::host;
use super::range;
use crate::claude_cli::{BoardMcp, GuardHook};
use crate::hook::{SettingsOptions, hook_settings_opts};

/// The settings sources `claude` knows (`--setting-sources`).
pub const SOURCES: [&str; 3] = ["user", "project", "local"];

/// What the profiles need besides the request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchConfig {
    /// Settings files a controlled run loads.
    pub controlled_sources: Vec<String>,
    /// Settings files a reviewer loads.
    pub reviewer_sources: Vec<String>,
    /// Servers a controlled run may use besides the Board's.
    pub mcp: Vec<McpServerDef>,
}

impl Default for LaunchConfig {
    fn default() -> LaunchConfig {
        LaunchConfig {
            controlled_sources: vec!["user".into()],
            reviewer_sources: Vec::new(),
            mcp: Vec::new(),
        }
    }
}

/// Everything that shapes the arguments of a launch besides the request:
/// the guard's hook, the Board's server and the profile settings. The
/// runner and the runtime are given the same one, so what is recorded is
/// what was run.
#[derive(Clone, Debug)]
pub struct LaunchSpec {
    pub guard: Option<GuardHook>,
    pub board: Option<BoardMcp>,
    pub config: LaunchConfig,
    /// The PreToolUse hook carries `onFailure: "block"`.
    pub on_failure_block: bool,
    /// swarmux's status line is set in the generated settings.
    pub statusline: bool,
}

impl Default for LaunchSpec {
    fn default() -> LaunchSpec {
        LaunchSpec {
            guard: None,
            board: None,
            config: LaunchConfig::default(),
            on_failure_block: false,
            // As it has always been: swarmux's status line is set.
            statusline: true,
        }
    }
}

/// What the probes found out about this Claude Code and this user.
#[derive(Clone, Debug, Default)]
pub struct Probes {
    pub version: Option<Version>,
    pub host: swarmux_domain::runtime::HostPolicy,
    /// The user's own settings define a status line.
    pub user_statusline: bool,
}

impl Probes {
    /// Runs `claude --version` and reads the host's managed policy and the
    /// user's settings.
    pub fn run(bin: &std::path::Path) -> Probes {
        Probes {
            version: claude_version(bin).and_then(|v| Version::parse(&v)),
            host: host::probe(),
            user_statusline: host::user_config_dir().is_some_and(|d| host::user_has_statusline(&d)),
        }
    }
}

/// What `claude --version` prints, or `None` when it cannot be run.
pub fn claude_version(bin: &std::path::Path) -> Option<String> {
    let mut cmd = std::process::Command::new(bin);
    cmd.arg("--version");
    crate::process::hide_std(&mut cmd);
    let out = cmd.output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

impl LaunchSpec {
    /// A spec for `settings` and what the probes found. `onFailure` is set
    /// unless the version is known to predate it or the user said off; the
    /// status line is replaced only when the user has none or said on.
    pub fn resolve(mut self, settings: &RuntimeSettings, probes: &Probes) -> LaunchSpec {
        self.config = LaunchConfig {
            controlled_sources: settings.controlled_sources.clone(),
            reviewer_sources: settings.reviewer_sources.clone(),
            mcp: settings.mcp.clone(),
        };
        self.on_failure_block = match settings.hook_failure {
            HookFailure::Block => true,
            HookFailure::Off => false,
            HookFailure::Auto => probes.version.is_none_or(range::knows_on_failure),
        };
        self.statusline = match settings.statusline {
            StatuslineMode::On => true,
            StatuslineMode::Off => false,
            StatuslineMode::Auto => !probes.user_statusline,
        };
        self
    }

    /// The generated `--settings` JSON for a run, or `None` with no guard.
    pub fn settings_json(&self, controlled: bool) -> Option<String> {
        let guard = self.guard.as_ref()?;
        Some(hook_settings_opts(
            &guard.exe,
            guard.timeout_secs,
            &SettingsOptions {
                on_failure_block: self.on_failure_block,
                statusline: self.statusline,
                pin_hooks_on: controlled,
            },
        ))
    }

    /// The arguments after the profile's: the guard's `--settings` and the
    /// MCP servers.
    pub fn extra_args(&self, req: &RunRequest) -> Vec<String> {
        let controlled = is_controlled(req);
        let mut args = Vec::new();
        if let Some(settings) = self.settings_json(controlled) {
            args.extend(["--settings".to_string(), settings]);
        }
        args.extend(self.mcp_args(req, controlled));
        args
    }

    fn mcp_args(&self, req: &RunRequest, controlled: bool) -> Vec<String> {
        let board = self.board.as_ref().filter(|_| req.board);
        // A reviewer has no servers but the Board's, if it is on one.
        let defs: &[McpServerDef] = if controlled && !req.review {
            &self.config.mcp
        } else {
            &[]
        };
        match board {
            Some(board) if controlled => {
                let json = board.config_json(req.job_id);
                let merged = controlled_mcp_config(Some(&json), defs).unwrap_or(json);
                vec![
                    "--mcp-config".into(),
                    merged,
                    "--allowedTools".into(),
                    crate::board::SERVER_NAME_PREFIX.into(),
                ]
            }
            Some(board) => board.args(req.job_id).to_vec(),
            None => controlled_mcp_config(None, defs)
                .map(|json| vec!["--mcp-config".to_string(), json])
                .unwrap_or_default(),
        }
    }
}

/// Whether the run is controlled: a reviewer always is.
pub fn is_controlled(req: &RunRequest) -> bool {
    req.review || req.profile == RuntimeProfile::Controlled
}

/// The value of `--setting-sources` for `req`, or `None` for the default
/// (every source). An empty value loads none.
pub fn setting_sources(req: &RunRequest, cfg: &LaunchConfig) -> Option<String> {
    if req.review {
        return Some(cfg.reviewer_sources.join(","));
    }
    match req.profile {
        RuntimeProfile::Controlled => {
            // A lean session never loads the user's own settings.
            let kept: Vec<&str> = cfg
                .controlled_sources
                .iter()
                .map(String::as_str)
                .filter(|s| !(req.lean && *s == "user"))
                .collect();
            Some(kept.join(","))
        }
        RuntimeProfile::Inherit => req.lean.then(|| "project,local".to_string()),
    }
}

/// `--mcp-config` JSON for a controlled run: the Board's server (when the
/// job is on a board) and the servers the user allowed. `None` when there
/// is neither.
pub fn controlled_mcp_config(board_json: Option<&str>, defs: &[McpServerDef]) -> Option<String> {
    let mut servers = Map::new();
    if let Some(board) = board_json
        && let Ok(Value::Object(mut root)) = serde_json::from_str::<Value>(board)
        && let Some(Value::Object(found)) = root.remove("mcpServers")
    {
        servers.extend(found);
    }
    for def in defs {
        // The Board's name is swarmux's own; a configured server never
        // replaces it.
        servers
            .entry(def.name.clone())
            .or_insert_with(|| server_json(def));
    }
    (!servers.is_empty()).then(|| json!({ "mcpServers": servers }).to_string())
}

fn server_json(def: &McpServerDef) -> Value {
    let env: Map<String, Value> = def
        .env
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();
    match &def.url {
        Some(url) => json!({ "type": def.transport, "url": url }),
        None => {
            let mut v = json!({ "type": "stdio", "command": def.command, "args": def.args });
            if !env.is_empty() {
                v["env"] = Value::Object(env);
            }
            v
        }
    }
}

/// How the sources read in a record: `default` for all of them, else the
/// list as passed (`user`, or empty for none).
pub fn describe_sources(sources: Option<&str>) -> String {
    match sources {
        None => "default".to_string(),
        Some("") => "none".to_string(),
        Some(list) => list.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_domain::{JobId, SessionId};
    use uuid::Uuid;

    fn req(profile: RuntimeProfile, lean: bool, review: bool) -> RunRequest {
        RunRequest {
            job_id: JobId(1),
            cwd: "/w".into(),
            session_id: SessionId(Uuid::nil()),
            resume: false,
            fork: false,
            model: "sonnet".into(),
            permission_mode: "default".into(),
            prompt: "hi".into(),
            lean,
            review,
            env: Vec::new(),
            max_budget_usd: None,
            board: false,
            add_dirs: Vec::new(),
            profile,
        }
    }

    #[test]
    fn inherit_loads_everything_unless_lean() {
        let cfg = LaunchConfig::default();
        assert_eq!(
            setting_sources(&req(RuntimeProfile::Inherit, false, false), &cfg),
            None
        );
        assert_eq!(
            setting_sources(&req(RuntimeProfile::Inherit, true, false), &cfg).as_deref(),
            Some("project,local")
        );
    }

    #[test]
    fn controlled_leaves_out_the_repository() {
        let cfg = LaunchConfig::default();
        assert_eq!(
            setting_sources(&req(RuntimeProfile::Controlled, false, false), &cfg).as_deref(),
            Some("user")
        );
        // Lean and controlled together load no settings file at all.
        assert_eq!(
            setting_sources(&req(RuntimeProfile::Controlled, true, false), &cfg).as_deref(),
            Some("")
        );
    }

    #[test]
    fn a_reviewer_is_controlled_whatever_it_is_asked() {
        let cfg = LaunchConfig::default();
        let r = req(RuntimeProfile::Inherit, false, true);
        assert!(is_controlled(&r));
        assert_eq!(setting_sources(&r, &cfg).as_deref(), Some(""));
        let cfg = LaunchConfig {
            reviewer_sources: vec!["user".into()],
            ..cfg
        };
        assert_eq!(setting_sources(&r, &cfg).as_deref(), Some("user"));
    }

    fn def(name: &str) -> McpServerDef {
        McpServerDef {
            name: name.into(),
            command: "npx".into(),
            args: vec!["-y".into(), "docs-server".into()],
            env: vec![("TOKEN".into(), "t".into())],
            url: None,
            transport: "stdio".into(),
        }
    }

    #[test]
    fn the_mcp_config_has_the_board_and_the_allowed_servers_only() {
        assert_eq!(controlled_mcp_config(None, &[]), None);
        let board = r#"{"mcpServers":{"swarmux-board":{"type":"stdio","command":"swarmux","args":["board-mcp"]}}}"#;
        let merged: Value =
            serde_json::from_str(&controlled_mcp_config(Some(board), &[def("docs")]).unwrap())
                .unwrap();
        let servers = merged["mcpServers"].as_object().unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers["swarmux-board"]["command"], "swarmux");
        assert_eq!(servers["docs"]["command"], "npx");
        assert_eq!(servers["docs"]["env"]["TOKEN"], "t");
        // A server named like the Board's never replaces it.
        let clash: Value = serde_json::from_str(
            &controlled_mcp_config(Some(board), &[def("swarmux-board")]).unwrap(),
        )
        .unwrap();
        assert_eq!(clash["mcpServers"]["swarmux-board"]["command"], "swarmux");
    }

    #[test]
    fn a_remote_server_is_passed_by_address() {
        let remote = McpServerDef {
            name: "wiki".into(),
            command: String::new(),
            args: Vec::new(),
            env: Vec::new(),
            url: Some("https://wiki.example/mcp".into()),
            transport: "http".into(),
        };
        let v: Value =
            serde_json::from_str(&controlled_mcp_config(None, &[remote]).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["wiki"]["type"], "http");
        assert_eq!(v["mcpServers"]["wiki"]["url"], "https://wiki.example/mcp");
    }

    #[test]
    fn sources_read_plainly_in_a_record() {
        assert_eq!(describe_sources(None), "default");
        assert_eq!(describe_sources(Some("")), "none");
        assert_eq!(describe_sources(Some("user")), "user");
    }
}
