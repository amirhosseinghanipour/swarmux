//! `swarmux keys`: list the hotkeys, check the `[keys]` of the config, and
//! export the keys in use.

use std::process::ExitCode;

use anyhow::anyhow;
use swarmux_infrastructure::{Paths, load_effective_settings};
use swarmux_tui::keys::{Effective, effective};
use swarmux_tui::keys_cli;

/// What `swarmux keys` was asked to do.
#[derive(Clone, Copy)]
pub enum Action {
    List,
    Lint,
    Export,
}

/// The keymap in use, whether the config has a `[keys]` section, and where
/// the config is.
fn load() -> anyhow::Result<(Effective, bool, String)> {
    let paths = Paths::default_home()?;
    let settings = load_effective_settings(&paths);
    let found = effective(&settings.keys).map_err(|e| anyhow!("{e}"))?;
    Ok((
        found,
        !settings.keys.is_empty(),
        paths.config.display().to_string(),
    ))
}

pub fn run(action: Action) -> anyhow::Result<ExitCode> {
    let (found, configured, source) = load()?;
    match action {
        Action::List => print!("{}", keys_cli::list(&found)),
        Action::Export => print!("{}", keys_cli::export(&found)),
        Action::Lint => {
            let (report, failed) = keys_cli::lint(&found, &source, configured);
            print!("{report}");
            let unreadable = Paths::default_home()
                .map(|p| crate::configcmd::report_syntax(&p))
                .unwrap_or(false);
            if failed || unreadable {
                return Ok(ExitCode::FAILURE);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
