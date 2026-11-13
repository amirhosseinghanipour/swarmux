//! Keys on the terminal side: turning crossterm events into [`KeySpec`]s,
//! the actions and contexts that belong to no single screen, and the
//! default keymap, which is what the screens declare put together.
//!
//! Nothing outside this module and the screens' own binding tables names a
//! literal key. The app asks the [`Keymap`] what a press means and runs the
//! action it names.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use swarmux_domain::keys::{
    Action, BindError, Catalog, Chord, Context, GLOBAL, Key, KeyConfig, KeyError, KeyProblem,
    KeySpec, Keymap, Mods, Shadow, actions, apply, new_shadows,
};

use crate::screens::{self, Bind, PLANNED, Screen, Section};

// ---- contexts

/// Answers to whatever is asking the user: a proposed plan, the JEV
/// consent, a held tool call. Active on every screen, before the screen's
/// own keys; an action that has nothing to answer lets the key through.
pub const DECIDE: Context = Context::new("decide");
/// The choice after `s` on an inbox item.
pub const SHELVE: Context = Context::new("shelve");
/// The key list that `?` opens: what scrolls it. Closing it is the global
/// `help.toggle` (and `Esc`).
pub const HELP: Context = Context::new("help");

// ---- actions shared by several screens, or by none

pub const DECIDE_YES: Action = Action::new("decide.yes");
pub const DECIDE_NO: Action = Action::new("decide.no");
pub const DECIDE_ALWAYS: Action = Action::new("decide.always");
pub const DECIDE_CANCEL_PLAN: Action = Action::new("decide.cancel_plan");

pub const SHELVE_15_MIN: Action = Action::new("shelve.15_min");
pub const SHELVE_60_MIN: Action = Action::new("shelve.60_min");
pub const SHELVE_UNTIL_JOB_ENDS: Action = Action::new("shelve.until_job_ends");

/// Opens the list of keys, or closes it.
pub const HELP_TOGGLE: Action = Action::new("help.toggle");
pub const HELP_PAGE_UP: Action = Action::new("help.page_up");
pub const HELP_PAGE_DOWN: Action = Action::new("help.page_down");

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

/// Scrolling the key list.
const HELP_SECTION: Section = Section {
    context: HELP,
    binds: &[
        bind(actions::LIST_PREV, &["up", "k"], "scroll up"),
        bind(actions::LIST_NEXT, &["down", "j"], "scroll down"),
        bind(HELP_PAGE_UP, &["pageup"], "page up"),
        bind(HELP_PAGE_DOWN, &["pagedown"], "page down"),
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
        bind(HELP_TOGGLE, &["?"], "show the keys of this screen"),
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
    all.push(HELP_SECTION);
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
    // What answers an approval must stay where the user expects it, and
    // Esc is not a binding at all.
    for answer in [DECIDE_YES, DECIDE_NO, DECIDE_ALWAYS, DECIDE_CANCEL_PLAN] {
        catalog.lock(DECIDE, answer);
    }
    catalog.lock(GLOBAL, actions::BACK);
    Ok((keymap, catalog))
}

/// The keys of `action` in `context` as a person reads them: `esc` for
/// going back, `ctrl+c` always among the ways to quit, `(none)` when
/// unbound.
pub fn keys_text(keymap: &Keymap, context: Context, action: Action) -> String {
    if action == actions::BACK {
        return "esc".to_string();
    }
    let mut keys: Vec<String> = keymap
        .chords_of(context, action)
        .map(ToString::to_string)
        .collect();
    if action == actions::QUIT {
        keys.push("ctrl+c".to_string());
    }
    if keys.is_empty() {
        "(none)".to_string()
    } else {
        keys.join(", ")
    }
}

/// The keymap in use once the user's `[keys]` is applied, with what was
/// wrong with it. Never fails because of the config: what can't be used
/// is reported and the defaults stand in for it.
pub struct Effective {
    pub keymap: Keymap,
    pub catalog: Catalog,
    /// The keys before the config, for what a config changes.
    pub defaults: Keymap,
    /// Entries that were refused. Each one's action keeps its default keys.
    pub problems: Vec<KeyProblem>,
    /// Overlaps the config made: legal, and probably not meant.
    pub warnings: Vec<Shadow>,
}

/// Every stack of contexts the app can have at once, as [`crate::app::App`]
/// builds them: the decision overlays, the screen's own, the global ones.
fn context_stacks() -> Vec<Vec<Context>> {
    let mut out = Vec::new();
    for screen in screens::all() {
        for inner in screen.def().context_stacks() {
            let mut stack = vec![DECIDE];
            stack.extend(inner);
            stack.push(GLOBAL);
            out.push(stack);
        }
    }
    out.push(vec![HELP, GLOBAL]);
    out
}

/// The defaults with `config` applied.
pub fn effective(config: &KeyConfig) -> Result<Effective, DefaultsError> {
    let (defaults, catalog) = defaults()?;
    let applied = apply(&defaults, &catalog, config);
    let warnings = new_shadows(&defaults, &applied.keymap, &context_stacks());
    Ok(Effective {
        keymap: applied.keymap,
        catalog,
        defaults,
        problems: applied.problems,
        warnings,
    })
}

/// The key that goes to `screen`, as the tab bar shows it.
pub fn nav_label(keymap: &Keymap, screen: Screen) -> String {
    keymap
        .label_of(GLOBAL, screen.def().nav_action())
        .unwrap_or_else(|| "·".to_string())
}

/// A `[keys]` section from `(section, name, keys)` lines, for tests.
#[cfg(test)]
pub(crate) fn config_of(entries: &[(&str, &str, &[&str])]) -> KeyConfig {
    KeyConfig {
        entries: entries
            .iter()
            .map(|(section, name, keys)| swarmux_domain::keys::KeyEntry {
                section: section.to_string(),
                name: name.to_string(),
                keys: keys.iter().map(|k| k.to_string()).collect(),
            })
            .collect(),
        malformed: Vec::new(),
    }
}

#[cfg(test)]
mod rebind_tests;
#[cfg(test)]
mod tests;
