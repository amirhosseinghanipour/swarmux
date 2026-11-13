//! Missions: outcomes in flight, and the composer that writes them.

use crossterm::event::KeyEvent;
use ratatui::{Frame, layout::Rect};
use swarmux_application::AppResult;
use swarmux_domain::keys::{Action, Context, actions};

use super::{Bind, Handled, Hint, Screen, ScreenDef, Section};
use crate::app::App;
use crate::keys::{DECIDE_CANCEL_PLAN, DECIDE_NO, DECIDE_YES};
use crate::missions::Composer;

pub(super) struct Def;

const CTX: Context = Context::new("missions");

pub const NEW: Action = Action::new("missions.new");
pub const EDIT: Action = Action::new("missions.edit");
pub const LINT: Action = Action::new("missions.lint");
pub const FIX: Action = Action::new("missions.fix");
pub const RUN: Action = Action::new("missions.run");
pub const PLAN_FIRST: Action = Action::new("missions.plan_first");

const SECTIONS: &[Section] = &[Section {
    context: CTX,
    binds: &[
        Bind {
            action: actions::LIST_PREV,
            keys: &["up", "k"],
            help: "previous mission",
        },
        Bind {
            action: actions::LIST_NEXT,
            keys: &["down", "j"],
            help: "next mission",
        },
        Bind {
            action: NEW,
            keys: &["n"],
            help: "write a new mission",
        },
        Bind {
            action: EDIT,
            keys: &["e"],
            help: "edit the mission",
        },
        Bind {
            action: LINT,
            keys: &["l"],
            help: "lint the contract",
        },
        Bind {
            action: FIX,
            keys: &["f"],
            help: "protect the oracles the lint found",
        },
        Bind {
            action: RUN,
            keys: &["r"],
            help: "run the mission",
        },
        Bind {
            action: PLAN_FIRST,
            keys: &["p"],
            help: "plan before building, or not",
        },
        Bind {
            action: actions::LIST_OPEN,
            keys: &["enter"],
            help: "follow the mission's job in focus",
        },
    ],
}];

/// The footer.
const HINTS: &[Hint] = &[
    Hint::keys(&[NEW], "new"),
    Hint::keys(&[EDIT], "edit"),
    Hint::keys(&[LINT], "lint"),
    Hint::keys(&[FIX], "protect"),
    Hint::keys(&[PLAN_FIRST], "plan first"),
    Hint::keys(&[RUN], "run"),
    Hint::keys(&[DECIDE_YES, DECIDE_NO, DECIDE_CANCEL_PLAN], "plan"),
    Hint::symbols(&[actions::LIST_OPEN], "focus"),
    Hint::keys(&[actions::QUIT], "quit"),
];

/// The footer while the composer is open: its keys are typed as they are.
const COMPOSER_HINTS: &[Hint] = &[
    Hint::fixed("tab", "next field"),
    Hint::fixed("ctrl+s", "save & lint"),
    Hint::fixed("ctrl+f", "protect oracles"),
    Hint::fixed("ctrl+r", "save & run"),
    Hint::fixed("esc", "close"),
];

impl ScreenDef for Def {
    fn id(&self) -> Screen {
        Screen::Missions
    }

    fn title(&self) -> &'static str {
        "missions"
    }

    fn nav_action(&self) -> Action {
        actions::GO_MISSIONS
    }

    fn nav_keys(&self) -> &'static [&'static str] {
        &["2", "g m"]
    }

    fn nav_help(&self) -> &'static str {
        "go to missions"
    }

    fn sections(&self) -> &'static [Section] {
        SECTIONS
    }

    fn contexts(&self, _app: &App) -> Vec<Context> {
        vec![CTX]
    }

    fn draw(&self, frame: &mut Frame, area: Rect, app: &App) {
        crate::ui::missions::render(frame, area, app);
    }

    fn hints(&self, app: &App) -> &'static [Hint] {
        if app.missions_screen.composer.is_some() {
            COMPOSER_HINTS
        } else {
            HINTS
        }
    }

    fn context_stacks(&self) -> Vec<Vec<Context>> {
        vec![vec![CTX]]
    }

    fn handle(&self, app: &mut App, action: Action) -> Handled {
        let count = app.missions().len();
        let slug = app.selected_mission().map(|m| m.slug.clone());
        let result: AppResult<()> = match action {
            actions::LIST_PREV => {
                app.missions_screen.move_by(-1, count);
                Ok(())
            }
            actions::LIST_NEXT => {
                app.missions_screen.move_by(1, count);
                Ok(())
            }
            NEW => {
                if app.engine.missions.is_none() {
                    app.notice = Some("missions are not set up in this run".into());
                } else if app.sessions.is_empty() {
                    app.error = Some(app.need_session());
                } else {
                    app.missions_screen.composer = Some(Composer::blank());
                }
                Ok(())
            }
            EDIT => {
                if let Some(m) = app.selected_mission() {
                    if m.state.in_flight() {
                        app.notice =
                            Some(format!("{} is {}: it can't change now", m.slug, m.state));
                    } else {
                        app.missions_screen.composer = Some(Composer::edit(m));
                    }
                }
                Ok(())
            }
            LINT => match (&mut app.engine.missions, slug) {
                (Some(service), Some(slug)) => service.lint(&slug).map(drop),
                _ => Ok(()),
            },
            FIX => match slug {
                Some(slug) => app.fix_mission(&slug),
                None => Ok(()),
            },
            RUN => match slug {
                Some(slug) => app.launch_mission(&slug),
                None => Ok(()),
            },
            PLAN_FIRST => match slug {
                Some(slug) => app.toggle_plan_first(&slug),
                None => Ok(()),
            },
            actions::LIST_OPEN => match app.selected_mission().and_then(|m| m.job) {
                Some(job) => app.view(job),
                None => {
                    app.notice = Some("that mission has not run yet: r runs it".into());
                    Ok(())
                }
            },
            _ => return Handled::No,
        };
        app.show(result);
        Handled::Yes
    }

    /// The composer is a text editor: its keys are typed, not bound.
    fn edit_key(&self, app: &mut App, key: KeyEvent) -> bool {
        let Some(composer) = &mut app.missions_screen.composer else {
            return false;
        };
        app.error = None;
        app.notice = None;
        let action = composer.handle(key);
        let result = app.composer_action(action);
        app.show(result);
        true
    }
}
