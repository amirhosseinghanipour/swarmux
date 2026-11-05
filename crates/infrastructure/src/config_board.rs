//! The `[board]` section of `config.toml`: the Board of a mission's agents.

use serde::Deserialize;
use swarmux_application::BoardSettings;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawBoard {
    enabled: Option<bool>,
}

pub(crate) fn parse_board(raw: RawBoard) -> BoardSettings {
    let mut settings = BoardSettings::default();
    if let Some(on) = raw.enabled {
        settings.enabled = on;
    }
    settings
}
