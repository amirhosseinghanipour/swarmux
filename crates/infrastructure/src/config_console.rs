//! The `[console]` section of `config.toml`: what the Work and Deck
//! screens show.

use std::path::Path;

use serde::Deserialize;
use swarmux_application::ConsoleSettings;
use swarmux_application::MAX_SEND_BACK_NOTES;
use swarmux_application::metrics::MAX_HOURS;

use crate::config::{ConfigError, key_line};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawConsole {
    metrics_hours: Option<i64>,
    send_back_notes: Option<Vec<String>>,
}

pub(crate) fn parse_console(
    raw: RawConsole,
    text: &str,
    path: &Path,
) -> Result<ConsoleSettings, ConfigError> {
    let mut settings = ConsoleSettings::default();
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
