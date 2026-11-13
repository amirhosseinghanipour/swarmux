//! Knowledge: rules, facts and decisions kept for the agents, and the
//! contracts between repositories.

use ratatui::{Frame, layout::Rect};
use swarmux_domain::keys::{Action, Context, actions};

use super::{Bind, Handled, Hint, Screen, ScreenDef, Section};
use crate::app::App;

pub(super) struct Def;

/// Both tabs.
const CTX: Context = Context::new("knowledge");
const ITEMS: Context = Context::new("knowledge.items");
const CONTRACTS: Context = Context::new("knowledge.contracts");

pub const TOGGLE_CONTRACTS: Action = Action::new("knowledge.toggle_contracts");
pub const ADD: Action = Action::new("knowledge.add");
pub const EDIT: Action = Action::new("knowledge.edit");
pub const PROMOTE: Action = Action::new("knowledge.promote");
pub const CONFIRM: Action = Action::new("knowledge.confirm");
pub const RETIRE: Action = Action::new("knowledge.retire");
pub const REREAD_CONTRACTS: Action = Action::new("knowledge.reread_contracts");
pub const NEXT_WORKSPACE: Action = Action::new("knowledge.next_workspace");

const SECTIONS: &[Section] = &[
    Section {
        context: CTX,
        binds: &[Bind {
            action: TOGGLE_CONTRACTS,
            keys: &["t"],
            help: "items or the contracts between repositories",
        }],
    },
    Section {
        context: ITEMS,
        binds: &[
            Bind {
                action: actions::LIST_PREV,
                keys: &["up", "k"],
                help: "previous item",
            },
            Bind {
                action: actions::LIST_NEXT,
                keys: &["down", "j"],
                help: "next item",
            },
            Bind {
                action: ADD,
                keys: &["a"],
                help: "add an item",
            },
            Bind {
                action: EDIT,
                keys: &["e"],
                help: "edit the item's text",
            },
            Bind {
                action: PROMOTE,
                keys: &["p"],
                help: "promote the item",
            },
            Bind {
                action: CONFIRM,
                keys: &["c"],
                help: "confirm a stale item",
            },
            Bind {
                action: RETIRE,
                keys: &["x"],
                help: "retire the item",
            },
        ],
    },
    Section {
        context: CONTRACTS,
        binds: &[
            Bind {
                action: actions::LIST_PREV,
                keys: &["up", "k"],
                help: "previous contract",
            },
            Bind {
                action: actions::LIST_NEXT,
                keys: &["down", "j"],
                help: "next contract",
            },
            Bind {
                action: REREAD_CONTRACTS,
                keys: &["r"],
                help: "read the contracts again",
            },
            Bind {
                action: NEXT_WORKSPACE,
                keys: &["w"],
                help: "next workspace",
            },
        ],
    },
];

/// The footer.
const HINTS: &[Hint] = &[
    Hint::symbols(&[actions::LIST_PREV, actions::LIST_NEXT], "select"),
    Hint::keys(&[ADD], "add (kind: text)"),
    Hint::keys(&[EDIT], "edit"),
    Hint::keys(&[PROMOTE], "promote"),
    Hint::keys(&[CONFIRM], "confirm"),
    Hint::keys(&[RETIRE], "retire"),
    Hint::Screens,
    Hint::keys(&[actions::QUIT], "quit"),
];

impl ScreenDef for Def {
    fn id(&self) -> Screen {
        Screen::Knowledge
    }

    fn title(&self) -> &'static str {
        "knowledge"
    }

    fn nav_action(&self) -> Action {
        actions::GO_KNOWLEDGE
    }

    fn nav_keys(&self) -> &'static [&'static str] {
        &["8", "g k"]
    }

    fn nav_help(&self) -> &'static str {
        "go to knowledge"
    }

    fn sections(&self) -> &'static [Section] {
        SECTIONS
    }

    fn contexts(&self, app: &App) -> Vec<Context> {
        let tab = if app.knowledge_screen.contracts_tab {
            CONTRACTS
        } else {
            ITEMS
        };
        vec![CTX, tab]
    }

    fn draw(&self, frame: &mut Frame, area: Rect, app: &App) {
        crate::ui::knowledge::render(frame, area, app);
    }

    fn hints(&self, _app: &App) -> &'static [Hint] {
        HINTS
    }

    fn context_stacks(&self) -> Vec<Vec<Context>> {
        vec![vec![CTX, ITEMS], vec![CTX, CONTRACTS]]
    }

    fn handle(&self, app: &mut App, action: Action) -> Handled {
        app.knowledge_action(action)
    }
}
