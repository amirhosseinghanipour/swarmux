//! Hotkeys as data: key presses, the actions they trigger, and the keymap
//! and resolver that connect them. Pure rules; reading a terminal's events
//! and running the actions is the interface's job.
//!
//! Nothing that reacts to the user may match on a literal key. It names an
//! [`Action`], and a [`Keymap`] (defaults in code, overrides from the user's
//! config) says which keys trigger it. `Esc` and `Ctrl+C` are reserved: they
//! can't be rebound, so there is always a way back and out.

mod config;
mod keymap;
mod spec;

pub use config::{
    Applied, KeyConfig, KeyEntry, KeyProblem, Malformed, apply, key_name, new_shadows, table_name,
};
pub use keymap::{
    Action, ActionDef, BindError, Binding, CHORD_TIMEOUT_MS, Catalog, Context, GLOBAL, Keymap,
    Lookup, Outcome, Resolver, Shadow, actions,
};
pub use spec::{Chord, Key, KeyError, KeySpec, MAX_CHORD_LEN, Mods, is_reserved};

#[cfg(test)]
mod config_tests;
#[cfg(test)]
mod tests;
