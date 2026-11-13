//! Keys on the terminal side: turning crossterm events into [`KeySpec`]s,
//! the actions and contexts that belong to no single screen, and the
//! default keymap, which is what the screens declare put together.
//!
//! Nothing outside this module and the screens' own binding tables names a
//! literal key. The app asks the [`Keymap`] what a press means and runs the
//! action it names.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use swarmux_domain::keys::{
    Action, BindError, Catalog, Chord, Context, GLOBAL, Key, KeyError, KeySpec, Keymap, Mods,
    actions,
};

use crate::screens::{self, Bind, PLANNED, Screen, Section};

// ---- contexts

/// Answers to whatever is asking the user: a proposed plan, the JEV
/// consent, a held tool call. Active on every screen, before the screen's
/// own keys; an action that has nothing to answer lets the key through.
pub const DECIDE: Context = Context::new("decide");
/// The choice after `s` on an inbox item.
pub const SHELVE: Context = Context::new("shelve");

// ---- actions shared by several screens, or by none

pub const DECIDE_YES: Action = Action::new("decide.yes");
pub const DECIDE_NO: Action = Action::new("decide.no");
pub const DECIDE_ALWAYS: Action = Action::new("decide.always");
pub const DECIDE_CANCEL_PLAN: Action = Action::new("decide.cancel_plan");

pub const SHELVE_15_MIN: Action = Action::new("shelve.15_min");
pub const SHELVE_60_MIN: Action = Action::new("shelve.60_min");
pub const SHELVE_UNTIL_JOB_ENDS: Action = Action::new("shelve.until_job_ends");

pub const LOOK_NEXT_THEME: Action = Action::new("look.next_theme");
pub const LOOK_NEXT_LAYOUT: Action = Action::new("look.next_layout");
pub const INBOX_FOCUS_MODE: Action = Action::new("inbox.focus_mode");
pub const SHOWCASE_CONSTELLATION: Action = Action::new("showcase.constellation");
pub const SHOWCASE_SPECTRUM: Action = Action::new("showcase.spectrum");

/// Type a prompt for the selected session.
pub const PROMPT_SEND: Action = Action::new("prompt.send");
/// Type a prompt swarmux sends to the session it picks.
pub const PROMPT_ROUTE: Action = Action::new("prompt.route");
/// Answer the selected question on the board.
pub const BOARD_ANSWER: Action = Action::new("board.answer");

const fn bind(action: Action, keys: &'static [&'static str], help: &'static str) -> Bind {
    Bind { action, keys, help }
}

/// Bindings of the overlays that answer the user's decisions.
const DECIDE_SECTION: Section = Section {
    context: DECIDE,
    binds: &[
        bind(DECIDE_YES, &["y"], "approve what is asking"),
        bind(DECIDE_NO, &["n"], "deny it, or send a plan back"),
        bind(DECIDE_ALWAYS, &["a"], "always allow the held call"),
        bind(DECIDE_CANCEL_PLAN, &["c"], "cancel the proposed plan"),
    ],
};

const SHELVE_SECTION: Section = Section {
    context: SHELVE,
    binds: &[
        bind(SHELVE_15_MIN, &["1"], "shelve for 15 minutes"),
        bind(SHELVE_60_MIN, &["2"], "shelve for 60 minutes"),
        bind(SHELVE_UNTIL_JOB_ENDS, &["3"], "shelve until its job ends"),
    ],
};

/// What works on every screen, after the screen's own keys.
const GLOBAL_SECTION: Section = Section {
    context: GLOBAL,
    binds: &[
        bind(actions::QUIT, &["q"], "leave swarmux (Ctrl+C always does)"),
        bind(
            actions::BACK,
            &[],
            "dismiss what is open, else go back (Esc)",
        ),
        bind(LOOK_NEXT_THEME, &["t"], "next theme"),
        bind(LOOK_NEXT_LAYOUT, &["L"], "next layout"),
        bind(INBOX_FOCUS_MODE, &["F"], "focus mode on or off"),
        bind(SHOWCASE_CONSTELLATION, &["z"], "the constellation"),
        bind(SHOWCASE_SPECTRUM, &["S"], "the spectrum"),
    ],
};

/// The key an event stands for, or none for keys swarmux has no name for.
///
/// Shift is part of a character (`D` arrives as `D`, with or without a
/// shift flag), and Ctrl+Alt on a character is how Windows reports AltGr:
/// the character is already what the user typed (`[` on a German layout),
/// so the modifiers are dropped.
pub fn spec_of(event: KeyEvent) -> Option<KeySpec> {
    let mut mods = Mods::NONE;
    if event.modifiers.contains(KeyModifiers::CONTROL) {
        mods = mods | Mods::CTRL;
    }
    if event.modifiers.contains(KeyModifiers::ALT) {
        mods = mods | Mods::ALT;
    }
    if event.modifiers.contains(KeyModifiers::SHIFT) {
        mods = mods | Mods::SHIFT;
    }
    let key = match event.code {
        KeyCode::Char(c) => {
            if mods.contains(Mods::CTRL) && mods.contains(Mods::ALT) {
                mods = Mods::NONE;
            }
            Key::Char(c)
        }
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Delete => Key::Delete,
        KeyCode::Insert => Key::Insert,
        KeyCode::F(n) => Key::F(n),
        _ => return None,
    };
    Some(KeySpec::new(key, mods))
}

/// Why the default keymap could not be built. It is code, so this is a bug
/// that a test catches; the message says which binding.
#[derive(Debug, PartialEq, Eq)]
pub struct DefaultsError(String);

impl std::fmt::Display for DefaultsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the default keymap is invalid: {}", self.0)
    }
}

impl std::error::Error for DefaultsError {}

impl From<BindError> for DefaultsError {
    fn from(e: BindError) -> DefaultsError {
        DefaultsError(e.to_string())
    }
}

/// The sections of every context, in the order they are searched at
/// build time: the screens', then the overlays', then the global one.
fn sections() -> Vec<Section> {
    let mut all: Vec<Section> = Vec::new();
    for screen in screens::all() {
        all.extend_from_slice(screen.def().sections());
    }
    all.push(DECIDE_SECTION);
    all.push(SHELVE_SECTION);
    all.push(GLOBAL_SECTION);
    all
}

/// The default keymap, and the catalog of every action it can bind.
///
/// This reproduces the keys the screens have always had, plus `g` and a
/// letter to go to a screen.
pub fn defaults() -> Result<(Keymap, Catalog), DefaultsError> {
    let mut keymap = Keymap::new();
    let mut catalog = Catalog::new();
    let read = |context: Context, action: Action, keys: &[&str]| {
        keys.iter()
            .map(|k| Chord::parse(k))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e: KeyError| DefaultsError(format!("{context}: {action}: {e}")))
    };
    for section in sections() {
        for b in section.binds {
            catalog.add(section.context, b.action, b.help);
            keymap.bind_all(
                section.context,
                read(section.context, b.action, b.keys)?,
                b.action,
            )?;
        }
    }
    // Going to a screen: its own keys, in the global context.
    for screen in screens::all() {
        let def = screen.def();
        catalog.add(GLOBAL, def.nav_action(), def.nav_help());
        keymap.bind_all(
            GLOBAL,
            read(GLOBAL, def.nav_action(), def.nav_keys())?,
            def.nav_action(),
        )?;
    }
    for planned in PLANNED {
        catalog.add(GLOBAL, planned.action, planned.help);
        keymap.bind_all(
            GLOBAL,
            read(GLOBAL, planned.action, planned.keys)?,
            planned.action,
        )?;
    }
    Ok((keymap, catalog))
}

/// The key that goes to `screen`, as the tab bar shows it.
pub fn nav_label(keymap: &Keymap, screen: Screen) -> String {
    keymap
        .label_of(GLOBAL, screen.def().nav_action())
        .unwrap_or_else(|| "·".to_string())
}

#[cfg(test)]
mod tests;
