//! Deck: the task graph with what the work needs around it. It reads the
//! console snapshot; opening a card hands it to Work.

use ratatui::{Frame, layout::Rect};
use swarmux_domain::keys::{Action, Context, actions};

use super::{Bind, Changes, Handled, Hint, Screen, ScreenDef, Section};
use crate::app::App;
use crate::deck::Toward;
use crate::keys::HELP_TOGGLE;

pub(super) struct Def;

const CTX: Context = Context::new("deck");

pub const PRODUCER: Action = Action::new("deck.producer");
pub const CONSUMER: Action = Action::new("deck.consumer");
pub const SESSIONS: Action = Action::new("deck.sessions");
pub const MORE: Action = Action::new("deck.more");

const SECTIONS: &[Section] = &[Section {
    context: CTX,
    binds: &[
        Bind {
            action: actions::LIST_PREV,
            keys: &["up", "k"],
            help: "previous card (or session row, in the sessions strip)",
        },
        Bind {
            action: actions::LIST_NEXT,
            keys: &["down", "j"],
            help: "next card (or session row, in the sessions strip)",
        },
        Bind {
            action: PRODUCER,
            keys: &["left", "h"],
            help: "go to a card this one waits for",
        },
        Bind {
            action: CONSUMER,
            keys: &["right", "l"],
            help: "go to a card that waits for this one",
        },
        Bind {
            action: actions::LIST_OPEN,
            keys: &["enter"],
            help: "open the card's outcome in Work (or the session's job)",
        },
        Bind {
            action: SESSIONS,
            keys: &["s"],
            help: "move between the graph and the sessions strip",
        },
        Bind {
            action: MORE,
            keys: &["tab"],
            help: "at a small size: bring the next folded panel in place of the graph",
        },
    ],
}];

/// The footer.
const HINTS: &[Hint] = &[
    Hint::symbols(&[actions::LIST_PREV, actions::LIST_NEXT], "select"),
    Hint::symbols(&[PRODUCER, CONSUMER], "follow"),
    Hint::symbols(&[actions::LIST_OPEN], "open in Work"),
    Hint::keys(&[SESSIONS], "sessions"),
    Hint::keys(&[MORE], "more"),
    Hint::keys(&[HELP_TOGGLE], "help"),
    Hint::keys(&[actions::QUIT], "quit"),
];

impl ScreenDef for Def {
    fn id(&self) -> Screen {
        Screen::Deck
    }

    fn title(&self) -> &'static str {
        "deck"
    }

    fn nav_action(&self) -> Action {
        actions::GO_DECK
    }

    fn nav_keys(&self) -> &'static [&'static str] {
        &["g d"]
    }

    fn nav_help(&self) -> &'static str {
        "go to the deck: the task graph and what the work needs"
    }

    fn sections(&self) -> &'static [Section] {
        SECTIONS
    }

    fn contexts(&self, _app: &App) -> Vec<Context> {
        vec![CTX]
    }

    fn context_stacks(&self) -> Vec<Vec<Context>> {
        vec![vec![CTX]]
    }

    fn draw(&self, frame: &mut Frame, area: Rect, app: &App) {
        crate::ui::deck::render(frame, area, app);
    }

    fn hints(&self, _app: &App) -> &'static [Hint] {
        HINTS
    }

    fn handle(&self, app: &mut App, action: Action) -> Handled {
        match action {
            actions::LIST_PREV => app.deck_step(-1),
            actions::LIST_NEXT => app.deck_step(1),
            PRODUCER => return app.deck_toward(Toward::Producer).into(),
            CONSUMER => return app.deck_toward(Toward::Consumer).into(),
            actions::LIST_OPEN => app.deck_open(),
            SESSIONS => app.deck_sessions(),
            MORE => app.deck_more(),
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn enter(&self, app: &mut App) {
        let _ = app.console_snapshot();
    }

    fn tick(&self, _app: &mut App, _changes: Changes) {}
}
