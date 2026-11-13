//! The screens as a registry.
//!
//! A screen is one [`ScreenDef`]: its title, the keys that go to it, the
//! keys it understands (as named actions, in its own contexts), how it
//! draws, runs an action and refreshes. The app knows no screen by name:
//! it asks the registry. Adding a screen is a file here with its definition
//! and one line in [`all`]; nothing in `app.rs` changes.

use crossterm::event::KeyEvent;
use ratatui::{Frame, layout::Rect, text::Line};
use swarmux_domain::keys::{Action, Context, actions};

use crate::app::App;
use crate::theme::Theme;

pub(crate) mod board;
pub(crate) mod dashboard;
pub(crate) mod followups;
pub(crate) mod knowledge;
pub(crate) mod ledger;
pub(crate) mod mission;
pub(crate) mod missions;
pub(crate) mod prompts;
pub(crate) mod queue;
pub(crate) mod sessions;

/// Names a screen. Cheap to copy and compare; what it can do is in its
/// [`ScreenDef`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Screen(&'static str);

/// Names for the screens that exist today.
#[allow(non_upper_case_globals)]
impl Screen {
    /// Mission control: the home screen.
    pub const Mission: Screen = Screen::new("mission");
    /// Outcomes in flight: mission cards and the composer.
    pub const Missions: Screen = Screen::new("missions");
    pub const Prompts: Screen = Screen::new("prompts");
    pub const Queue: Screen = Screen::new("queue");
    /// What happened, what it cost and whether it can be proven.
    pub const Ledger: Screen = Screen::new("ledger");
    pub const Sessions: Screen = Screen::new("sessions");
    /// The followed job in detail.
    pub const Dashboard: Screen = Screen::new("focus");
    /// What the agents of each mission wrote to each other.
    pub const Board: Screen = Screen::new("board");
    /// Rules, facts and decisions kept for the agents.
    pub const Knowledge: Screen = Screen::new("knowledge");
    /// What agents left behind, as decision cards.
    pub const Followups: Screen = Screen::new("followups");
}

impl Screen {
    const fn new(id: &'static str) -> Screen {
        Screen(id)
    }

    pub fn id(self) -> &'static str {
        self.0
    }

    /// The screen's definition.
    pub fn def(self) -> &'static dyn ScreenDef {
        REGISTRY
            .iter()
            .copied()
            .find(|d| d.id() == self)
            .expect("every screen is in the registry")
    }

    pub fn title(self) -> &'static str {
        self.def().title()
    }

    /// The screen the app starts on.
    pub const HOME: Screen = Screen::Mission;
}

impl Default for Screen {
    fn default() -> Screen {
        Screen::HOME
    }
}

/// Every screen, in the order the tabs list them. The registry.
const REGISTRY: &[&dyn ScreenDef] = &[
    &mission::Def,
    &missions::Def,
    &prompts::Def,
    &queue::Def,
    &ledger::Def,
    &sessions::Def,
    &dashboard::Def,
    &board::Def,
    &knowledge::Def,
    &followups::Def,
];

/// The screens, in tab order.
pub fn all() -> impl Iterator<Item = Screen> + Clone {
    REGISTRY.iter().map(|d| d.id())
}

/// The screen the navigation `action` goes to, for the screens that exist.
pub fn nav_target(action: Action) -> Option<Screen> {
    REGISTRY
        .iter()
        .find(|d| d.nav_action() == action)
        .map(|d| d.id())
}

/// What an action did with a key press: ran, or did not apply right now
/// (so the next candidate may try).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub enum Handled {
    Yes,
    No,
}

impl From<bool> for Handled {
    fn from(taken: bool) -> Handled {
        if taken { Handled::Yes } else { Handled::No }
    }
}

/// What changed since the last update, for the screens that refresh.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Jobs started, ended or were queued.
    pub jobs: bool,
    /// A bundle was verified or a label arrived.
    pub evidence: bool,
}

/// One key (or more) for an action, with what it does.
#[derive(Clone, Copy, Debug)]
pub struct Bind {
    pub action: Action,
    /// Keys as a config writes them (`"x"`, `"ctrl+s"`, `"g w"`).
    pub keys: &'static [&'static str],
    pub help: &'static str,
}

/// The default bindings of one context.
#[derive(Clone, Copy, Debug)]
pub struct Section {
    pub context: Context,
    pub binds: &'static [Bind],
}

/// One entry of a screen's footer. The keys are looked up in the live
/// keymap when it is drawn, so a rebound key shows as what it is now, and
/// an action with no key shows nothing.
#[derive(Clone, Copy, Debug)]
pub enum Hint {
    /// The first key of each action, joined, then what they do.
    Keys {
        actions: &'static [Action],
        text: &'static str,
        /// Arrows and Enter as symbols (`↑↓`, `⏎`) instead of words.
        symbols: bool,
    },
    /// A key that an editor takes as typed, bypassing the keymap.
    Fixed {
        keys: &'static str,
        text: &'static str,
    },
    /// The keys that go to the screens, as a range (`0-9`).
    Screens,
}

impl Hint {
    /// `x text`, `x/y text`.
    pub const fn keys(actions: &'static [Action], text: &'static str) -> Hint {
        Hint::Keys {
            actions,
            text,
            symbols: false,
        }
    }

    /// Like [`Hint::keys`], with arrows and Enter as symbols.
    pub const fn symbols(actions: &'static [Action], text: &'static str) -> Hint {
        Hint::Keys {
            actions,
            text,
            symbols: true,
        }
    }

    pub const fn fixed(keys: &'static str, text: &'static str) -> Hint {
        Hint::Fixed { keys, text }
    }
}

/// A screen that is planned but not built. Its navigation keys already
/// work: they go to the nearest screen that exists, and say so.
pub struct Planned {
    pub action: Action,
    pub keys: &'static [&'static str],
    pub help: &'static str,
    /// What it is called.
    pub name: &'static str,
    /// Where its keys go until it exists; none, they only say so.
    pub stand_in: Option<Screen>,
}

/// The planned screens. Each line is a promise that the key works and
/// that the user is told what it did.
pub const PLANNED: &[Planned] = &[
    Planned {
        action: actions::GO_DECK,
        keys: &["g d"],
        help: "go to the deck",
        name: "deck",
        stand_in: Some(Screen::Mission),
    },
    Planned {
        action: actions::GO_WORK,
        keys: &["g w"],
        help: "go to work",
        name: "work",
        stand_in: Some(Screen::Queue),
    },
    Planned {
        action: actions::GO_RECORD,
        keys: &["g r"],
        help: "go to the record",
        name: "record",
        stand_in: Some(Screen::Ledger),
    },
];

/// What one screen is and does.
pub trait ScreenDef: Sync {
    fn id(&self) -> Screen;

    /// The tab's label.
    fn title(&self) -> &'static str;

    /// The action that goes to this screen.
    fn nav_action(&self) -> Action;

    /// The keys that go to it, by default.
    fn nav_keys(&self) -> &'static [&'static str];

    fn nav_help(&self) -> &'static str {
        "go to this screen"
    }

    /// The screen's default keys, by context.
    fn sections(&self) -> &'static [Section];

    /// The contexts whose keys apply now, innermost first. The overlays
    /// and the global context are added by the app.
    fn contexts(&self, app: &App) -> Vec<Context>;

    fn draw(&self, frame: &mut Frame, area: Rect, app: &App);

    /// Every stack of contexts the screen can have, innermost first (what
    /// [`ScreenDef::contexts`] gives in each state of the screen). The
    /// overlays and the global context are added by the caller.
    fn context_stacks(&self) -> Vec<Vec<Context>>;

    /// The footer, most important first, the way out last. Which keys they
    /// show comes from the keymap; see [`crate::hints`].
    fn hints(&self, app: &App) -> &'static [Hint];

    /// Runs `action`, or says it does not apply. Actions that do nothing
    /// useful now (nothing selected, a feature off) return [`Handled::No`]
    /// only when the key should reach whatever comes next.
    fn handle(&self, app: &mut App, action: Action) -> Handled;

    /// Compatibility path for a screen that edits text: while its editor is
    /// open every key goes to it as typed, bypassing the keymap. Returns
    /// whether the editor took the key.
    fn edit_key(&self, _app: &mut App, _key: KeyEvent) -> bool {
        false
    }

    /// The user came to this screen.
    fn enter(&self, _app: &mut App) {}

    /// Once per update, while this screen shows.
    fn tick(&self, _app: &mut App, _changes: Changes) {}

    /// The constellation and the spectrum may open from here.
    fn allows_showcase(&self) -> bool {
        false
    }

    /// A header of its own in place of the title.
    fn header(&self, _app: &App, _t: &Theme) -> Option<Line<'static>> {
        None
    }

    /// A status line of its own in place of the hints.
    fn status(&self, _app: &App, _t: &Theme) -> Option<Line<'static>> {
        None
    }

    /// The tab bar shows the theme and layout on its right.
    fn shows_look(&self, _app: &App) -> bool {
        false
    }
}

#[cfg(test)]
mod tests;
