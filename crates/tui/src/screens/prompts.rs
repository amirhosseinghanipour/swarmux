//! Prompts: the saved templates, and sending one to a session.

use crossterm::event::KeyEvent;
use ratatui::{Frame, layout::Rect};
use swarmux_domain::keys::{Action, Context, actions};

use super::{Bind, Handled, Screen, ScreenDef, Section};
use crate::app::App;
use crate::prompts::{FormAction, PromptForm};

pub(super) struct Def;

const CTX: Context = Context::new("prompts");

pub const NEW: Action = Action::new("prompts.new");
pub const EDIT: Action = Action::new("prompts.edit");
pub const DELETE: Action = Action::new("prompts.delete");
pub const DISPATCH: Action = Action::new("prompts.dispatch");
pub const ROUTE: Action = Action::new("prompts.route");

const SECTIONS: &[Section] = &[Section {
    context: CTX,
    binds: &[
        Bind {
            action: NEW,
            keys: &["n"],
            help: "write a new prompt",
        },
        Bind {
            action: EDIT,
            keys: &["e"],
            help: "edit the selected prompt",
        },
        Bind {
            action: DELETE,
            keys: &["x"],
            help: "delete the selected prompt (press twice)",
        },
        Bind {
            action: DISPATCH,
            keys: &["d"],
            help: "send it to the selected session",
        },
        Bind {
            action: ROUTE,
            keys: &["D"],
            help: "send it to the session swarmux picks",
        },
        Bind {
            action: actions::LIST_PREV,
            keys: &["up"],
            help: "previous prompt",
        },
        Bind {
            action: actions::LIST_NEXT,
            keys: &["down"],
            help: "next prompt",
        },
    ],
}];

impl ScreenDef for Def {
    fn id(&self) -> Screen {
        Screen::Prompts
    }

    fn title(&self) -> &'static str {
        "prompts"
    }

    fn nav_action(&self) -> Action {
        actions::GO_PROMPTS
    }

    fn nav_keys(&self) -> &'static [&'static str] {
        &["3", "g p"]
    }

    fn nav_help(&self) -> &'static str {
        "go to prompts"
    }

    fn sections(&self) -> &'static [Section] {
        SECTIONS
    }

    fn contexts(&self, _app: &App) -> Vec<Context> {
        vec![CTX]
    }

    fn draw(&self, frame: &mut Frame, area: Rect, app: &App) {
        crate::ui::prompts::render(frame, area, app);
    }

    fn hints(&self, app: &App) -> &'static str {
        if app.prompts.form.is_some() {
            return " tab next field · ctrl+s save · esc cancel";
        }
        " n new · e edit · x delete · d dispatch · D route · ↑↓ select · 0-9 screens · q quit"
    }

    fn handle(&self, app: &mut App, action: Action) -> Handled {
        match action {
            actions::LIST_PREV => app.prompts.up(),
            actions::LIST_NEXT => app.prompts.down(),
            NEW => app.prompts.form = Some(PromptForm::blank()),
            EDIT => {
                if let Some(p) = app.prompts.selected_prompt() {
                    app.prompts.form = Some(PromptForm::edit(p));
                }
            }
            DELETE => {
                if let Some(id) = app.prompts.selected_prompt().map(|p| p.id) {
                    if app.prompts.delete_armed {
                        let result = app.prompt_service.delete(id);
                        if result.is_ok() {
                            app.refresh_prompts(None);
                        }
                        app.show(result);
                    } else {
                        app.prompts.confirm_delete = true;
                    }
                }
            }
            DISPATCH | ROUTE => {
                let result = app.start_template_dispatch(action == ROUTE);
                app.show(result);
            }
            _ => return Handled::No,
        }
        Handled::Yes
    }

    /// The prompt form is a text editor: its keys are typed, not bound.
    fn edit_key(&self, app: &mut App, key: KeyEvent) -> bool {
        let Some(form) = &mut app.prompts.form else {
            return false;
        };
        app.error = None;
        match form.handle(key) {
            FormAction::None => {}
            FormAction::Cancel => app.prompts.form = None,
            FormAction::Save => {
                let result = app.save_form();
                app.show(result);
            }
        }
        true
    }
}
