//! The `[console]` section of `config.toml`: what the Work and Deck
//! screens show.

use std::path::Path;

use serde::Deserialize;
use swarmux_application::ConsoleSettings;
use swarmux_application::MAX_SEND_BACK_NOTES;
use swarmux_application::metrics::MAX_HOURS;
use swarmux_domain::intervene::{MAX_MAX_CHARS, Mode};

use crate::config::{ConfigError, key_line};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawConsole {
    metrics_hours: Option<i64>,
    send_back_notes: Option<Vec<String>>,
    // Read as plain values: a wrong one is reported and the default kept,
    // so a typo here never keeps swarmux from starting.
    intervene_mode: Option<toml::Value>,
    intervene_max_chars: Option<toml::Value>,
    intervene_confirm_steer: Option<toml::Value>,
}

pub(crate) fn parse_console(
    raw: RawConsole,
    text: &str,
    path: &Path,
    warnings: &mut Vec<String>,
) -> Result<ConsoleSettings, ConfigError> {
    let mut settings = ConsoleSettings::default();
    parse_intervene(&raw, text, &mut settings, warnings);
    if let Some(hours) = raw.metrics_hours {
        settings.metrics_hours = u32::try_from(hours)
            .ok()
            .filter(|h| (1..=MAX_HOURS).contains(h))
            .ok_or_else(|| ConfigError {
                path: path.to_path_buf(),
                line: key_line(text, "metrics_hours"),
                message: format!(
                    "metrics_hours must be between 1 and {MAX_HOURS} (how many hours back the verification figures look), not {hours}"
                ),
            })?;
    }
    // The user's own list replaces the default one, empty included.
    if let Some(notes) = raw.send_back_notes {
        let error = |message: String| ConfigError {
            path: path.to_path_buf(),
            line: key_line(text, "send_back_notes"),
            message,
        };
        if notes.len() > MAX_SEND_BACK_NOTES {
            return Err(error(format!(
                "send_back_notes may list at most {MAX_SEND_BACK_NOTES} notes, not {}",
                notes.len()
            )));
        }
        let mut kept = Vec::new();
        for note in notes {
            let note = note.trim();
            if note.is_empty() {
                return Err(error("send_back_notes must not hold an empty note".into()));
            }
            if note.chars().count() > MAX_NOTE_CHARS {
                return Err(error(format!(
                    "a note in send_back_notes may be {MAX_NOTE_CHARS} characters at most"
                )));
            }
            kept.push(note.to_string());
        }
        settings.send_back_notes = kept;
    }
    Ok(settings)
}

/// Longest suggested note, in characters.
const MAX_NOTE_CHARS: usize = 400;

/// The `intervene_*` keys. A value that is not usable is reported (the
/// lint message names the key and the line) and the default is kept.
fn parse_intervene(
    raw: &RawConsole,
    text: &str,
    settings: &mut ConsoleSettings,
    warnings: &mut Vec<String>,
) {
    let say = |key: &str, why: String| {
        let line = key_line(text, key).map_or(String::new(), |l| format!(" (line {l})"));
        format!("config.toml{line}: [console] {key} {why}")
    };
    if let Some(v) = &raw.intervene_mode {
        match v.as_str().and_then(Mode::parse) {
            Some(mode) => settings.intervene.mode = mode,
            None => warnings.push(say(
                "intervene_mode",
                format!(
                    "must be \"queue\" or \"steer\", not {v}: using \"{}\"",
                    settings.intervene.mode.as_str()
                ),
            )),
        }
    }
    if let Some(v) = &raw.intervene_max_chars {
        match v
            .as_integer()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| (1..=MAX_MAX_CHARS).contains(n))
        {
            Some(n) => settings.intervene.max_chars = n,
            None => warnings.push(say(
                "intervene_max_chars",
                format!(
                    "must be a number of characters between 1 and {MAX_MAX_CHARS}, not {v}: using {}",
                    settings.intervene.max_chars
                ),
            )),
        }
    }
    if let Some(v) = &raw.intervene_confirm_steer {
        match v.as_bool() {
            Some(b) => settings.intervene.confirm_steer = b,
            None => warnings.push(say(
                "intervene_confirm_steer",
                format!(
                    "must be true or false, not {v}: using {}",
                    settings.intervene.confirm_steer
                ),
            )),
        }
    }
}
