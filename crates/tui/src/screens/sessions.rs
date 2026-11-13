//! Sessions: the conversations swarmux keeps, and the workspaces they work on.

use ratatui::{Frame, layout::Rect};
use swarmux_domain::keys::{Action, Context, actions};

use super::{Bind, Handled, Screen, ScreenDef, Section};
use crate::app::{App, InputPurpose, Mode};
use crate::ui::input::Input;

pub(super) struct Def;

const CTX: Context = Context::new("sessions");

pub const NEW: Action = Action::new("sessions.new");
pub const ON_WORKSPACE: Action = Action::new("sessions.on_workspace");
pub const TOGGLE_LEAN: Action = Action::new("sessions.toggle_lean");

const SECTIONS: &[Section] = &[Section {
    context: CTX,
    binds: &[
        Bind {
            action: NEW,
            keys: &["n"],
            help: "new session in the folder swarmux started in",
        },
        Bind {
            action: ON_WORKSPACE,
            keys: &["w"],
            help: "new session on a workspace",
        },
        Bind {
            action: TOGGLE_LEAN,
            keys: &["l"],
            help: "lean on or off for the session's next jobs",
        },
        Bind {
            action: actions::LIST_PREV,
            keys: &["up"],
            help: "previous session",
        },
        Bind {
            action: actions::LIST_NEXT,
            keys: &["down"],
            help: "next session",
        },
    ],
}];

impl ScreenDef for Def {
    fn id(&self) -> Screen {
        Screen::Sessions
    }

    fn title(&self) -> &'static str {
        "sessions"
    }

    fn nav_action(&self) -> Action {
        actions::GO_SESSIONS
    }

    fn nav_keys(&self) -> &'static [&'static str] {
        &["6"]
    }

    fn nav_help(&self) -> &'static str {
        "go to sessions"
    }

    fn sections(&self) -> &'static [Section] {
        SECTIONS
    }

    fn contexts(&self, _app: &App) -> Vec<Context> {
        vec![CTX]
    }

    fn draw(&self, frame: &mut Frame, area: Rect, app: &App) {
        crate::ui::sessions::render(frame, area, app);
    }

    fn hints(&self, _app: &App) -> &'static str {
        " n new session · w on a workspace · l lean on/off · ↑↓ select · 0-9 screens · q quit"
    }

    fn handle(&self, app: &mut App, action: Action) -> Handled {
        match action {
            NEW => app.mode = Mode::Input(InputPurpose::NewSession, Input::default()),
            ON_WORKSPACE => {
                if app.workspaces.is_empty() {
                    app.error =
                        Some("no workspaces: add one with `swarmux workspace add <folder>`".into());
                } else {
                    let first = app.workspaces[0].name.clone();
                    app.mode =
                        Mode::Input(InputPurpose::WorkspaceSession, Input::with_text(&first));
                }
            }
            TOGGLE_LEAN => {
                let result = app.toggle_lean();
                app.show(result);
            }
            actions::LIST_PREV => app.move_session(-1),
            actions::LIST_NEXT => app.move_session(1),
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn allows_showcase(&self) -> bool {
        true
    }
}
