//! Brief: what happened while the user was away and what waits for them.
//! It opens by itself after a gap (`[brief] away_after_minutes`) and on
//! `g b`; `Enter` starts with the first decision, `Esc` skips past it.

use ratatui::{Frame, layout::Rect};
use swarmux_domain::keys::{Action, Context, actions};

use super::{Bind, Changes, Handled, Hint, Screen, ScreenDef, Section};
use crate::app::App;
use crate::keys::HELP_TOGGLE;

pub(super) struct Def;

const CTX: Context = Context::new("brief");

pub const TO_WORK: Action = Action::new("brief.to_work");
pub const TO_DECK: Action = Action::new("brief.to_deck");
pub const MORE: Action = Action::new("brief.more");

const SECTIONS: &[Section] = &[Section {
    context: CTX,
    binds: &[
        Bind {
            action: actions::LIST_OPEN,
            keys: &["enter"],
            help: "start with the first decision, in Work",
        },
        Bind {
            action: TO_WORK,
            keys: &["w"],
            help: "skip the brief: go to Work",
        },
        Bind {
            action: TO_DECK,
            keys: &["d"],
            help: "skip the brief: go to the Deck",
        },
        Bind {
            action: MORE,
            keys: &["tab"],
            help: "at a small size: the sections that fold (what lands where, the timeline)",
        },
    ],
}];

const HINTS: &[Hint] = &[
    Hint::symbols(&[actions::LIST_OPEN], "first decision"),
    Hint::keys(&[TO_WORK], "skip to Work"),
    Hint::keys(&[TO_DECK], "skip to Deck"),
    Hint::keys(&[MORE], "more"),
    Hint::keys(&[HELP_TOGGLE], "help"),
    Hint::keys(&[actions::QUIT], "quit"),
];

impl ScreenDef for Def {
    fn id(&self) -> Screen {
        Screen::Brief
    }

    fn title(&self) -> &'static str {
        "brief"
    }

    fn nav_action(&self) -> Action {
        actions::GO_BRIEF
    }

    fn nav_keys(&self) -> &'static [&'static str] {
        &["g b"]
    }

    fn nav_help(&self) -> &'static str {
        "go to the brief: what happened while you were away"
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
        crate::ui::brief::render(frame, area, app);
    }

    fn hints(&self, _app: &App) -> &'static [Hint] {
        HINTS
    }

    fn handle(&self, app: &mut App, action: Action) -> Handled {
        match action {
            actions::LIST_OPEN => app.brief_open(),
            TO_WORK => app.skip_to(Screen::Work),
            TO_DECK => app.skip_to(Screen::Deck),
            MORE => app.brief_more(),
            // `Esc` is not a binding: it skips to what `[brief] esc_goes_to` says.
            actions::BACK => app.brief_skip(),
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn enter(&self, app: &mut App) {
        app.brief_refresh();
    }

    fn tick(&self, app: &mut App, _changes: Changes) {
        app.brief_refresh();
    }
}
