//! Board: what the agents of each mission wrote to each other.

use ratatui::{Frame, layout::Rect};
use swarmux_domain::keys::{Action, Context, actions};

use super::{Bind, Handled, Screen, ScreenDef, Section};
use crate::app::App;
use crate::keys::BOARD_ANSWER;

pub(super) struct Def;

const CTX: Context = Context::new("board");
/// Answering a question from the inbox of Mission control.
pub(super) const ASK: Context = Context::new("board.ask");

pub const DISMISS: Action = Action::new("board.dismiss");

const SECTIONS: &[Section] = &[
    Section {
        context: CTX,
        binds: &[
            Bind {
                action: actions::LIST_PREV,
                keys: &["up", "k"],
                help: "previous entry",
            },
            Bind {
                action: actions::LIST_NEXT,
                keys: &["down", "j"],
                help: "next entry",
            },
            Bind {
                action: BOARD_ANSWER,
                keys: &["r", "enter"],
                help: "answer the selected question",
            },
            Bind {
                action: DISMISS,
                keys: &["x"],
                help: "close the question without an answer",
            },
        ],
    },
    Section {
        context: ASK,
        binds: &[Bind {
            action: BOARD_ANSWER,
            keys: &["r", "enter"],
            help: "answer the selected question",
        }],
    },
];

impl ScreenDef for Def {
    fn id(&self) -> Screen {
        Screen::Board
    }

    fn title(&self) -> &'static str {
        "board"
    }

    fn nav_action(&self) -> Action {
        actions::GO_BOARD
    }

    fn nav_keys(&self) -> &'static [&'static str] {
        &["7"]
    }

    fn nav_help(&self) -> &'static str {
        "go to the board"
    }

    fn sections(&self) -> &'static [Section] {
        SECTIONS
    }

    fn contexts(&self, _app: &App) -> Vec<Context> {
        vec![CTX]
    }

    fn draw(&self, frame: &mut Frame, area: Rect, app: &App) {
        crate::ui::board::render(frame, area, app);
    }

    fn hints(&self, _app: &App) -> &'static str {
        " ↑↓ select · r answer a question · x close it · 0-9 screens · q quit"
    }

    fn handle(&self, app: &mut App, action: Action) -> Handled {
        match action {
            actions::LIST_PREV => {
                app.board_screen.selected = app.board_screen.selected.saturating_sub(1);
                Handled::Yes
            }
            actions::LIST_NEXT => {
                let count = app.board_entries().len();
                app.board_screen.selected =
                    (app.board_screen.selected + 1).min(count.saturating_sub(1));
                Handled::Yes
            }
            DISMISS => app.board_dismiss(),
            _ => Handled::No,
        }
    }
}
