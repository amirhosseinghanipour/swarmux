//! The `[notify]` section of `config.toml`.

use std::path::Path;

use serde::Deserialize;
use swarmux_application::{NotifySettings, QuietHours, TerminalMode};

use crate::config::{ConfigError, key_line};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawNotify {
    enabled: Option<bool>,
    toast: Option<bool>,
    terminal: Option<String>,
    quiet: Option<String>,
    ntfy: Option<String>,
}

pub(crate) fn parse_notify(
    raw: RawNotify,
    text: &str,
    path: &Path,
) -> Result<NotifySettings, ConfigError> {
    let error = |key: &str, message: &str| ConfigError {
        path: path.to_path_buf(),
        line: key_line(text, key),
        message: message.to_string(),
    };
    let mut n = NotifySettings::default();
    if let Some(on) = raw.enabled {
        n.enabled = on;
    }
    if let Some(on) = raw.toast {
        n.toast = on;
    }
    if let Some(mode) = raw.terminal {
        n.terminal = TerminalMode::parse(mode.trim()).ok_or_else(|| {
            error(
                "terminal",
                "terminal must be \"off\", \"osc777\", \"osc9\" or \"bell\"",
            )
        })?;
    }
    if let Some(q) = raw.quiet {
        let q = q.trim();
        n.quiet = if q.is_empty() || q == "off" {
            None
        } else {
            Some(QuietHours::parse(q).ok_or_else(|| {
                error(
                    "quiet",
                    "quiet must be a 24-hour window like \"22:00-08:00\"",
                )
            })?)
        };
    }
    if let Some(url) = raw.ntfy {
        let url = url.trim();
        let valid = (url.starts_with("https://") || url.starts_with("http://"))
            && !url.contains(char::is_whitespace)
            && url
                .split_once("://")
                .is_some_and(|(_, rest)| rest.contains('/'));
        n.ntfy = match url {
            "" => None,
            _ if valid => Some(url.to_string()),
            _ => {
                return Err(error(
                    "ntfy",
                    "ntfy must be a topic URL like \"https://ntfy.sh/my-topic\"",
                ));
            }
        };
    }
    Ok(n)
}
