//! The host policy probe: what Claude Code's managed settings say about
//! hooks, permission rules and MCP on this machine.
//!
//! Managed settings sit above everything swarmux passes with `--settings`,
//! so a lock there can make the Guard's hook not run at all. The probe reads
//! the places Claude Code documents (settings reference, managed-settings
//! page, read 2026-11-14): `managed-settings.json` and its `managed-settings.d`
//! folder, `managed-mcp.json`, and on Windows the `HKLM` and `HKCU` policy
//! keys. It does not read what Claude Code fetches from the server for
//! Team and Enterprise plans, nor the macOS managed-preferences domain: it
//! says so instead of reporting those as absent.
//!
//! A lock is reported when any document it found sets it. Claude Code ranks
//! its managed sources (the first with policy wins, unless the policy says
//! to merge), so this can report a lock that is not in force. That errs
//! toward refusing an unattended run, which is the side to err on.

use std::path::{Path, PathBuf};

use serde_json::Value;
use swarmux_domain::runtime::HostPolicy;

/// Where managed policy lives on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostPaths {
    /// The folder holding `managed-settings.json`, `managed-settings.d` and
    /// `managed-mcp.json`.
    pub dir: PathBuf,
    /// Registry values to read on Windows: `(label, key)`.
    pub registry: Vec<(String, String)>,
}

impl HostPaths {
    /// The documented locations for this operating system.
    pub fn system() -> HostPaths {
        let dir = if cfg!(windows) {
            PathBuf::from(r"C:\Program Files\ClaudeCode")
        } else if cfg!(target_os = "macos") {
            PathBuf::from("/Library/Application Support/ClaudeCode")
        } else {
            PathBuf::from("/etc/claude-code")
        };
        let registry = if cfg!(windows) {
            vec![
                (
                    "registry HKLM".to_string(),
                    r"HKLM\SOFTWARE\Policies\ClaudeCode".to_string(),
                ),
                (
                    "registry HKCU".to_string(),
                    r"HKCU\SOFTWARE\Policies\ClaudeCode".to_string(),
                ),
            ]
        } else {
            Vec::new()
        };
        HostPaths { dir, registry }
    }
}

/// Reads managed policy from `paths`. `registry_value` reads one registry
/// key's `Settings` value (`None` when it is not there).
pub fn probe_with(
    paths: &HostPaths,
    registry_value: &dyn Fn(&str) -> Option<String>,
) -> HostPolicy {
    let mut docs: Vec<(String, String)> = Vec::new();
    let mut unreadable: Vec<String> = Vec::new();
    let mut sources: Vec<String> = Vec::new();
    let mut managed_mcp = false;

    let file = paths.dir.join("managed-settings.json");
    read_doc(&file, &mut docs, &mut unreadable);
    let drop_ins = paths.dir.join("managed-settings.d");
    if let Ok(entries) = std::fs::read_dir(&drop_ins) {
        let mut names: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension().is_some_and(|x| x == "json")
                    && !p
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            })
            .collect();
        names.sort();
        for p in names {
            read_doc(&p, &mut docs, &mut unreadable);
        }
    }
    if paths.dir.join("managed-mcp.json").is_file() {
        managed_mcp = true;
        sources.push(format!(
            "{} (managed MCP servers)",
            paths.dir.join("managed-mcp.json").display()
        ));
    }
    for (label, key) in &paths.registry {
        if let Some(text) = registry_value(key) {
            docs.push((label.clone(), text));
        }
    }
    unreadable.push(
        "server-managed settings (Team and Enterprise plans) are fetched by Claude Code and are not read here; `claude doctor` reports them"
            .to_string(),
    );
    if cfg!(target_os = "macos") {
        unreadable.push(
            "the macOS managed-preferences domain com.anthropic.claudecode is not read".to_string(),
        );
    }
    analyze(docs, sources, unreadable, managed_mcp)
}

fn read_doc(path: &Path, docs: &mut Vec<(String, String)>, unreadable: &mut Vec<String>) {
    match std::fs::read_to_string(path) {
        Ok(text) => docs.push((path.display().to_string(), text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => unreadable.push(format!("{}: {e}", path.display())),
    }
}

fn add_lock(locks: &mut Vec<String>, name: &str) {
    if !locks.iter().any(|l| l == name) {
        locks.push(name.to_string());
    }
}

/// The locks in managed `docs` (`(source, JSON text)`), and what they do to
/// a run swarmux launches.
pub fn analyze(
    docs: Vec<(String, String)>,
    mut sources: Vec<String>,
    mut unreadable: Vec<String>,
    managed_mcp: bool,
) -> HostPolicy {
    let mut locks: Vec<String> = Vec::new();
    let mut blocks: Vec<String> = Vec::new();
    for (source, text) in docs {
        let value: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                // Claude Code refuses to start on an unparseable managed
                // document (managed-settings page).
                unreadable.push(format!("{source}: not valid JSON ({e})"));
                blocks.push(format!(
                    "{source} is not valid JSON; Claude Code refuses to start with an unparseable managed document"
                ));
                continue;
            }
        };
        sources.push(source);
        let on = |key: &str| value.get(key).and_then(Value::as_bool) == Some(true);
        if on("allowManagedHooksOnly") {
            add_lock(&mut locks, "allowManagedHooksOnly");
        }
        if on("allowManagedPermissionRulesOnly") {
            add_lock(&mut locks, "allowManagedPermissionRulesOnly");
        }
        if on("disableAllHooks") {
            add_lock(&mut locks, "disableAllHooks");
        }
        if on("allowManagedMcpServersOnly") {
            add_lock(&mut locks, "allowManagedMcpServersOnly");
        }
        if value
            .get("managedMcpServers")
            .and_then(Value::as_object)
            .is_some_and(|m| !m.is_empty())
        {
            add_lock(&mut locks, "managedMcpServers");
        }
    }
    if locks.iter().any(|l| l == "disableAllHooks") {
        blocks.push(
            "disableAllHooks is set in managed settings: every configured hook, including the Guard's, is off"
                .to_string(),
        );
    }
    if locks.iter().any(|l| l == "allowManagedHooksOnly") {
        blocks.push(
            "allowManagedHooksOnly is set: Claude Code's documentation lists only managed, SDK and force-enabled plugin hooks as still running, so the Guard's --settings hook is assumed dropped (not tested: that needs a managed file)"
                .to_string(),
        );
    }
    if managed_mcp {
        locks.push("managed-mcp.json".to_string());
        blocks.push(
            "managed-mcp.json exists: Claude Code's documentation says --mcp-config and --strict-mcp-config then make it exit at startup (not tested), which the Board and the controlled profile pass"
                .to_string(),
        );
    }
    HostPolicy {
        sources,
        locks,
        unreadable,
        blocks,
    }
}

/// Reads one registry key's `Settings` value with `reg query` (Windows).
pub fn registry_settings(key: &str) -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let mut cmd = std::process::Command::new("reg");
    cmd.args(["query", key, "/v", "Settings"]);
    crate::process::hide_std(&mut cmd);
    let out = cmd.output().ok().filter(|o| o.status.success())?;
    let text = String::from_utf8_lossy(&out.stdout);
    // `    Settings    REG_SZ    {json}`
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("Settings"))?;
    let after = line
        .split_once("REG_SZ")
        .or_else(|| line.split_once("REG_EXPAND_SZ"))?;
    Some(after.1.trim().to_string())
}

/// Probes this machine.
pub fn probe() -> HostPolicy {
    probe_with(&HostPaths::system(), &registry_settings)
}

/// Whether the user's own Claude Code settings define a status line.
pub fn user_has_statusline(config_dir: &Path) -> bool {
    std::fs::read_to_string(config_dir.join("settings.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .is_some_and(|v| v.get("statusLine").is_some())
}

/// The user's Claude Code folder: `CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub fn user_config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
    Some(PathBuf::from(home).join(".claude"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_in(dir: &Path) -> HostPolicy {
        probe_with(
            &HostPaths {
                dir: dir.to_path_buf(),
                registry: Vec::new(),
            },
            &|_| None,
        )
    }

    #[test]
    fn no_managed_files_means_no_locks_and_says_what_was_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let p = probe_in(dir.path());
        assert!(p.sources.is_empty() && p.locks.is_empty() && p.blocks.is_empty());
        assert!(p.unreadable.iter().any(|u| u.contains("server-managed")));
    }

    #[test]
    fn hooks_only_drops_the_guard_hook_and_is_named() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("managed-settings.json"),
            r#"{"allowManagedHooksOnly": true, "allowManagedPermissionRulesOnly": true}"#,
        )
        .unwrap();
        let p = probe_in(dir.path());
        assert_eq!(
            p.locks,
            ["allowManagedHooksOnly", "allowManagedPermissionRulesOnly"]
        );
        assert_eq!(p.blocks.len(), 1);
        assert!(p.blocks[0].contains("allowManagedHooksOnly"));
        assert_eq!(p.sources.len(), 1);
    }

    #[test]
    fn disable_all_hooks_in_a_drop_in_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join("managed-settings.d");
        std::fs::create_dir(&d).unwrap();
        std::fs::write(d.join("10-base.json"), r#"{"model":"x"}"#).unwrap();
        std::fs::write(d.join("20-lock.json"), r#"{"disableAllHooks": true}"#).unwrap();
        std::fs::write(d.join(".hidden.json"), r#"{"allowManagedHooksOnly": true}"#).unwrap();
        std::fs::write(d.join("notes.txt"), "allowManagedHooksOnly").unwrap();
        let p = probe_in(dir.path());
        assert_eq!(p.locks, ["disableAllHooks"]);
        assert!(p.blocks[0].contains("disableAllHooks"));
        assert_eq!(p.sources.len(), 2);
    }

    #[test]
    fn a_broken_managed_document_blocks_and_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("managed-settings.json"), "{ nope").unwrap();
        let p = probe_in(dir.path());
        assert!(p.blocks[0].contains("refuses to start"));
        assert!(p.unreadable.iter().any(|u| u.contains("not valid JSON")));
    }

    #[test]
    fn managed_mcp_blocks_the_mcp_flags() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("managed-mcp.json"), "{}").unwrap();
        let p = probe_in(dir.path());
        assert!(p.locks.contains(&"managed-mcp.json".to_string()));
        assert!(p.blocks[0].contains("--strict-mcp-config"));
    }

    #[test]
    fn a_registry_value_counts_like_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = probe_with(
            &HostPaths {
                dir: dir.path().to_path_buf(),
                registry: vec![("registry HKLM".into(), "HKLM\\K".into())],
            },
            &|_| Some(r#"{"allowManagedHooksOnly":true}"#.to_string()),
        );
        assert_eq!(p.locks, ["allowManagedHooksOnly"]);
        assert_eq!(p.sources, ["registry HKLM"]);
    }

    #[test]
    fn a_status_line_in_the_user_settings_is_seen() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!user_has_statusline(dir.path()));
        std::fs::write(dir.path().join("settings.json"), r#"{"theme":"dark"}"#).unwrap();
        assert!(!user_has_statusline(dir.path()));
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"statusLine":{"type":"command","command":"x"}}"#,
        )
        .unwrap();
        assert!(user_has_statusline(dir.path()));
    }
}
