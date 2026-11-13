//! Focus: the followed job in detail, or (classic) the dashboard's panels.

use ratatui::{Frame, layout::Rect, text::Line};
use swarmux_domain::keys::{Action, Context, actions};

use super::{Bind, Handled, Screen, ScreenDef, Section};
use crate::app::App;
use crate::keys::{PROMPT_ROUTE, PROMPT_SEND};
use crate::theme::Theme;

pub(super) struct Def;

/// Keys of both views.
const CTX: Context = Context::new("focus");
/// The reading view: the answer, the log and the diff.
const READING: Context = Context::new("focus.reading");
/// The classic panel grid.
const GRID: Context = Context::new("focus.grid");

pub const TOGGLE_CLASSIC: Action = Action::new("focus.toggle_classic");
pub const TOGGLE_REVIEW: Action = Action::new("focus.toggle_review");
pub const SCROLL_UP: Action = Action::new("focus.scroll_up");
pub const SCROLL_DOWN: Action = Action::new("focus.scroll_down");
pub const ANSWER_UP: Action = Action::new("focus.answer_up");
pub const ANSWER_DOWN: Action = Action::new("focus.answer_down");
pub const PAGE_UP: Action = Action::new("focus.page_up");
pub const PAGE_DOWN: Action = Action::new("focus.page_down");
pub const TOP: Action = Action::new("focus.top");
pub const BOTTOM: Action = Action::new("focus.bottom");
pub const TOGGLE_DIFF: Action = Action::new("focus.toggle_diff");
pub const CHECKPOINT_PREV: Action = Action::new("focus.checkpoint_prev");
pub const CHECKPOINT_NEXT: Action = Action::new("focus.checkpoint_next");
pub const OPEN_TERMINAL: Action = Action::new("focus.open_terminal");
pub const PANEL_NEXT: Action = Action::new("focus.panel_next");
pub const PANEL_PREV: Action = Action::new("focus.panel_prev");

const SECTIONS: &[Section] = &[
    Section {
        context: CTX,
        binds: &[
            Bind {
                action: TOGGLE_CLASSIC,
                keys: &["D"],
                help: "classic panels or the reading view",
            },
            Bind {
                action: TOGGLE_REVIEW,
                keys: &["v"],
                help: "review the next jobs when they finish",
            },
            Bind {
                action: PROMPT_SEND,
                keys: &["p"],
                help: "prompt the selected session",
            },
            Bind {
                action: PROMPT_ROUTE,
                keys: &["P"],
                help: "prompt; swarmux picks the session",
            },
        ],
    },
    Section {
        context: READING,
        binds: &[
            Bind {
                action: SCROLL_UP,
                keys: &["up", "k"],
                help: "scroll up",
            },
            Bind {
                action: SCROLL_DOWN,
                keys: &["down", "j"],
                help: "scroll down",
            },
            Bind {
                action: ANSWER_DOWN,
                keys: &["J"],
                help: "scroll the answer down",
            },
            Bind {
                action: ANSWER_UP,
                keys: &["K"],
                help: "scroll the answer up",
            },
            Bind {
                action: PAGE_UP,
                keys: &["pageup"],
                help: "page up",
            },
            Bind {
                action: PAGE_DOWN,
                keys: &["pagedown"],
                help: "page down",
            },
            Bind {
                action: TOP,
                keys: &["home"],
                help: "to the oldest line",
            },
            Bind {
                action: BOTTOM,
                keys: &["end", "G"],
                help: "back to the newest line",
            },
            Bind {
                action: TOGGLE_DIFF,
                keys: &["d"],
                help: "the uncommitted changes, or the answer",
            },
            Bind {
                action: CHECKPOINT_PREV,
                keys: &["["],
                help: "previous checkpoint",
            },
            Bind {
                action: CHECKPOINT_NEXT,
                keys: &["]"],
                help: "next checkpoint",
            },
            Bind {
                action: OPEN_TERMINAL,
                keys: &["t"],
                help: "open the session in a new terminal tab",
            },
        ],
    },
    Section {
        context: GRID,
        binds: &[
            Bind {
                action: PANEL_NEXT,
                keys: &["tab"],
                help: "focus the next panel",
            },
            Bind {
                action: PANEL_PREV,
                keys: &["shift+tab"],
                help: "focus the previous panel",
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
    },
];

impl ScreenDef for Def {
    fn id(&self) -> Screen {
        Screen::Dashboard
    }

    fn title(&self) -> &'static str {
        "focus"
    }

    fn nav_action(&self) -> Action {
        actions::GO_FOCUS
    }

    fn nav_keys(&self) -> &'static [&'static str] {
        &["0"]
    }

    fn nav_help(&self) -> &'static str {
        "go to focus"
    }

    fn sections(&self) -> &'static [Section] {
        SECTIONS
    }

    fn contexts(&self, app: &App) -> Vec<Context> {
        let view = if app.dashboard.classic { GRID } else { READING };
        vec![view, CTX]
    }

    fn draw(&self, frame: &mut Frame, area: Rect, app: &App) {
        if app.dashboard.classic {
            crate::ui::dashboard::render(frame, area, app);
        } else {
            crate::ui::focus::render(frame, area, app);
        }
    }

    fn hints(&self, app: &App) -> &'static str {
        if app.dashboard.classic {
            " p prompt · P route · v review · ↑↓ session · t theme · L layout · tab focus · q quit"
        } else {
            " ↑↓ scroll · d diff · t open session · p prompt · v review · D classic · q quit"
        }
    }

    fn handle(&self, app: &mut App, action: Action) -> Handled {
        match action {
            TOGGLE_CLASSIC => {
                app.dashboard.classic = !app.dashboard.classic;
                app.focus = None;
            }
            TOGGLE_REVIEW => {
                let on = !app.engine.jobs.review_next();
                app.notice = Some(if app.engine.jobs.set_review_next(on) {
                    format!(
                        "review on: the next jobs are audited by {} when they finish",
                        app.engine.jobs.review_model()
                    )
                } else if on {
                    "review is not available: no git workspace configured".into()
                } else {
                    "review off for the next jobs".into()
                });
            }
            SCROLL_UP => app.focus_state.scroll_by(1),
            SCROLL_DOWN => app.focus_state.scroll_by(-1),
            ANSWER_DOWN => app.focus_state.scroll_answer(3),
            ANSWER_UP => app.focus_state.scroll_answer(-3),
            PAGE_UP => app.focus_state.scroll_by(10),
            PAGE_DOWN => app.focus_state.scroll_by(-10),
            TOP => app.focus_state.scroll_by(isize::MAX),
            BOTTOM => app.focus_state.scroll = 0,
            TOGGLE_DIFF => {
                let result = app.toggle_diff();
                app.show(result);
            }
            CHECKPOINT_PREV => app.scrub_checkpoint(false),
            CHECKPOINT_NEXT => app.scrub_checkpoint(true),
            OPEN_TERMINAL => {
                let result = app.open_terminal();
                app.show(result);
            }
            PANEL_NEXT => app.cycle_focus(true),
            PANEL_PREV => app.cycle_focus(false),
            actions::LIST_PREV => app.move_session(-1),
            actions::LIST_NEXT => app.move_session(1),
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn allows_showcase(&self) -> bool {
        true
    }

    fn header(&self, app: &App, t: &Theme) -> Option<Line<'static>> {
        app.dashboard.classic.then(|| {
            crate::ui::dashboard::header::title(app, &crate::ui::dashboard::View::of(app), t)
        })
    }

    fn status(&self, app: &App, t: &Theme) -> Option<Line<'static>> {
        app.dashboard.classic.then(|| {
            crate::ui::dashboard::status::line(app, &crate::ui::dashboard::View::of(app), t)
        })
    }

    fn shows_look(&self, app: &App) -> bool {
        app.dashboard.classic
    }
}
