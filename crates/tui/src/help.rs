//! The `?` overlay: every key that works on the current screen, and the
//! global ones, written from the live keymap and the catalog of actions.

use std::cell::Cell;

use swarmux_domain::keys::{GLOBAL, table_name};

use crate::app::App;
use crate::keys::DECIDE;

/// What the overlay remembers: how far it is scrolled. Drawing clamps it
/// to what there is, so it is a cell.
#[derive(Debug, Default)]
pub(crate) struct HelpState {
    pub scroll: Cell<usize>,
}

/// One line of the overlay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HelpLine {
    /// A group of keys: its name, and the config table that rebinds them.
    Heading {
        title: String,
        table: String,
    },
    Row {
        /// The keys, as they are written in a config.
        keys: String,
        /// What it does.
        text: String,
        /// The action's id, as a config names it.
        id: String,
        /// The keys can't be changed.
        fixed: bool,
    },
    /// A sentence of its own.
    Note(String),
    Gap,
}

/// The overlay's lines for the screen the app is on.
pub(crate) fn lines(app: &App) -> Vec<HelpLine> {
    let mut out = Vec::new();
    if app.key_problems() > 0 {
        out.push(HelpLine::Note(format!(
            "{} problem(s) in the [keys] of config.toml; the default keys are used for them. \
             Run `swarmux keys lint`.",
            app.key_problems()
        )));
        out.push(HelpLine::Gap);
    }
    let mut groups = app.screen.def().contexts(app);
    groups.push(DECIDE);
    groups.push(GLOBAL);
    for context in groups {
        let title = if context == DECIDE {
            "answers (when something asks you)".to_string()
        } else if context == GLOBAL {
            "everywhere".to_string()
        } else {
            context.id().to_string()
        };
        out.push(HelpLine::Heading {
            title,
            table: format!("[keys.{}]", table_name(context.id())),
        });
        for def in app.catalog().of(context) {
            out.push(HelpLine::Row {
                keys: crate::keys::keys_text(app.keymap(), context, def.action),
                text: def.help.to_string(),
                id: def.action.id().to_string(),
                fixed: def.locked,
            });
        }
        out.push(HelpLine::Gap);
    }
    out.pop();
    out
}

#[cfg(test)]
mod tests;
