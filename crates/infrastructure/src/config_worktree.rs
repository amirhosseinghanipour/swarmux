//! The `[worktree]` section of `config.toml`.

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use swarmux_application::WorktreeSettings;
use swarmux_domain::PortPlan;

use crate::config::{ConfigError, key_line};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawWorktree {
    root: Option<String>,
    copy: Option<Vec<String>>,
    port_base: Option<i64>,
    port_block: Option<i64>,
    port_slots: Option<i64>,
    stale_hours: Option<i64>,
}

pub(crate) fn parse_worktree(
    raw: RawWorktree,
    text: &str,
    path: &Path,
) -> Result<WorktreeSettings, ConfigError> {
    let error = |key: &str, message: String| ConfigError {
        path: path.to_path_buf(),
        line: key_line(text, key),
        message,
    };
    let mut settings = WorktreeSettings::default();
    if let Some(root) = raw.root {
        let root = root.trim();
        if root.is_empty() {
            return Err(error("root", "worktree root can't be empty".into()));
        }
        settings.root = Some(PathBuf::from(root));
    }
    if let Some(copy) = raw.copy {
        for entry in &copy {
            let rel = Path::new(entry.trim());
            let inside = !entry.trim().is_empty()
                && !rel.is_absolute()
                && rel
                    .components()
                    .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
            if !inside {
                return Err(error(
                    "copy",
                    format!("copy entry {entry:?} must be a path inside the repository"),
                ));
            }
        }
        settings.copy = copy.iter().map(|e| e.trim().to_string()).collect();
    }
    let number = |key: &str, v: i64, lo: i64, hi: i64| -> Result<u16, ConfigError> {
        u16::try_from(v)
            .ok()
            .filter(|n| (lo..=hi).contains(&i64::from(*n)))
            .ok_or_else(|| error(key, format!("{key} must be between {lo} and {hi}")))
    };
    let mut plan: PortPlan = settings.ports;
    if let Some(v) = raw.port_base {
        plan.base = number("port_base", v, 1024, 65_535)?;
    }
    if let Some(v) = raw.port_block {
        plan.block = number("port_block", v, 1, 1000)?;
    }
    if let Some(v) = raw.port_slots {
        plan.slots = number("port_slots", v, 1, 5000)?;
    }
    if plan.port_base(u32::from(plan.slots) - 1).is_none() {
        return Err(error(
            "port_slots",
            "the port blocks run past 65535; lower port_base, port_block or port_slots".into(),
        ));
    }
    settings.ports = plan;
    if let Some(h) = raw.stale_hours {
        settings.stale_hours = u64::try_from(h)
            .ok()
            .filter(|h| *h >= 1)
            .ok_or_else(|| error("stale_hours", "stale_hours must be at least 1".into()))?;
    }
    Ok(settings)
}
