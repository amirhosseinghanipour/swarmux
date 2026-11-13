//! Mission control: the inbox of what needs the user, and the wall of
//! sessions.

use ratatui::{Frame, layout::Rect};
use swarmux_domain::keys::{Action, Context, actions};

use super::{Bind, Handled, Screen, ScreenDef, Section, board};
use crate::app::App;
use crate::keys::{PROMPT_ROUTE, PROMPT_SEND};
use crate::mission::Pane;

pub(super) struct Def;

const CTX: Context = Context::new("mission");
const WALL: Context = Context::new("mission.wall");
const INBOX: Context = Context::new("mission.inbox");

pub const SWITCH_PANE: Action = Action::new("mission.switch_pane");
pub const ACKNOWLEDGE: Action = Action::new("mission.acknowledge");
pub const ANSWER_BATCH: Action = Action::new("mission.answer_batch");
pub const SHELVE: Action = Action::new("mission.shelve");
pub const UNSHELVE: Action = Action::new("mission.unshelve");
pub const RULE_TAKE: Action = Action::new("mission.rule_take");
pub const RULE_DECLINE: Action = Action::new("mission.rule_decline");

const SECTIONS: &[Section] = &[
    Section {
        context: CTX,
        binds: &[
            Bind {
                action: SWITCH_PANE,
                keys: &["tab", "shift+tab"],
                help: "switch between the inbox and the wall",
            },
            Bind {
                action: ACKNOWLEDGE,
                keys: &["x"],
                help: "mark the item read (held calls stay)",
            },
            Bind {
                action: ANSWER_BATCH,
                keys: &["A"],
                help: "approve every ask of the selected one's kind",
            },
            Bind {
                action: SHELVE,
                keys: &["s"],
                help: "shelve the item for a while",
            },
            Bind {
                action: UNSHELVE,
                keys: &["u"],
                help: "bring shelved items back",
            },
            Bind {
                action: RULE_TAKE,
                keys: &["r"],
                help: "take the rule offered for the ask",
            },
            Bind {
                action: RULE_DECLINE,
                keys: &["R"],
                help: "turn the offered rule down",
            },
            Bind {
                action: actions::LIST_OPEN,
                keys: &["enter"],
                help: "open the item's job",
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
        context: WALL,
        binds: &[
            Bind {
                action: actions::LIST_PREV,
                keys: &["up", "left", "k", "h"],
                help: "previous session",
            },
            Bind {
                action: actions::LIST_NEXT,
                keys: &["down", "right", "j", "l"],
                help: "next session",
            },
        ],
    },
    Section {
        context: INBOX,
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
        ],
    },
];

impl ScreenDef for Def {
    fn id(&self) -> Screen {
        Screen::Mission
    }

    fn title(&self) -> &'static str {
        "mission"
    }

    fn nav_action(&self) -> Action {
        actions::GO_MISSION
    }

    fn nav_keys(&self) -> &'static [&'static str] {
        &["1"]
    }

    fn nav_help(&self) -> &'static str {
        "go to mission control"
    }

    fn sections(&self) -> &'static [Section] {
        SECTIONS
    }

    fn contexts(&self, app: &App) -> Vec<Context> {
        match app.mission.pane {
            // A question on the board is answered from the inbox.
            Pane::Inbox => vec![board::ASK, INBOX, CTX],
            Pane::Wall => vec![WALL, CTX],
        }
    }

    fn draw(&self, frame: &mut Frame, area: Rect, app: &App) {
        crate::ui::mission::render(frame, area, app);
    }

    fn hints(&self, _app: &App) -> &'static str {
        " ↑↓ select · ⏎ open · y/n/a answer · A batch · s shelve · F focus · tab wall · p prompt · q quit"
    }

    fn handle(&self, app: &mut App, action: Action) -> Handled {
        match action {
            SWITCH_PANE => {
                app.mission.pane = match app.mission.pane {
                    Pane::Inbox => Pane::Wall,
                    Pane::Wall => Pane::Inbox,
                };
            }
            actions::LIST_PREV | actions::LIST_NEXT => {
                let delta = if action == actions::LIST_PREV { -1 } else { 1 };
                if app.mission.pane == Pane::Wall {
                    app.move_wall(delta);
                } else {
                    app.mission.move_selection(delta);
                    let now = app.now();
                    app.mission.attend_selected(now);
                }
            }
            ACKNOWLEDGE => {
                let now = app.now();
                app.mission.attend_selected(now);
                if !app.mission.acknowledge_selected() {
                    app.notice = Some("held tool calls stay until you answer them".into());
                }
            }
            ANSWER_BATCH => {
                let result = app.answer_batch();
                app.show(result);
            }
            SHELVE => app.begin_shelve(),
            UNSHELVE => app.unshelve(),
            RULE_TAKE | RULE_DECLINE => {
                if app
                    .mission
                    .selected_item()
                    .is_none_or(|i| i.suggest.is_none())
                {
                    return Handled::No;
                }
                let result = app.settle_rule(action == RULE_TAKE);
                app.show(result);
            }
            actions::LIST_OPEN => {
                let now = app.now();
                app.mission.attend_selected(now);
                let result = app.open_selected();
                app.show(result);
            }
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn allows_showcase(&self) -> bool {
        true
    }
}
