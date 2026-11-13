//! Follow-ups: what agents left behind, as decision cards.

use ratatui::{Frame, layout::Rect};
use swarmux_domain::keys::{Action, Context, actions};

use super::{Bind, Changes, Handled, Hint, Screen, ScreenDef, Section};
use crate::app::App;
use crate::keys::HELP_TOGGLE;

pub(super) struct Def;

const CTX: Context = Context::new("followups");

pub const APPROVE: Action = Action::new("followups.approve");
pub const DECLINE: Action = Action::new("followups.decline");
pub const LATER: Action = Action::new("followups.later");
pub const OPEN_JOB: Action = Action::new("followups.open_job");
pub const DRAFT_MISSION: Action = Action::new("followups.draft_mission");

const SECTIONS: &[Section] = &[Section {
    context: CTX,
    binds: &[
        Bind {
            action: actions::LIST_PREV,
            keys: &["up", "k"],
            help: "previous card",
        },
        Bind {
            action: actions::LIST_NEXT,
            keys: &["down", "j"],
            help: "next card",
        },
        Bind {
            action: APPROVE,
            keys: &["a"],
            help: "approve the follow-up",
        },
        Bind {
            action: DECLINE,
            keys: &["d"],
            help: "decline it",
        },
        Bind {
            action: LATER,
            keys: &["l"],
            help: "ask again later",
        },
        Bind {
            action: OPEN_JOB,
            keys: &["o"],
            help: "open the job it came from",
        },
        Bind {
            action: DRAFT_MISSION,
            keys: &["m"],
            help: "draft a mission from it",
        },
    ],
}];

/// The footer.
const HINTS: &[Hint] = &[
    Hint::symbols(&[actions::LIST_PREV, actions::LIST_NEXT], "select"),
    Hint::keys(&[APPROVE], "approve"),
    Hint::keys(&[DECLINE], "decline"),
    Hint::keys(&[LATER], "later"),
    Hint::keys(&[OPEN_JOB], "open job"),
    Hint::keys(&[DRAFT_MISSION], "draft mission"),
    Hint::Screens,
    Hint::keys(&[HELP_TOGGLE], "help"),
    Hint::keys(&[actions::QUIT], "quit"),
];

impl ScreenDef for Def {
    fn id(&self) -> Screen {
        Screen::Followups
    }

    fn title(&self) -> &'static str {
        "follow-ups"
    }

    fn nav_action(&self) -> Action {
        actions::GO_FOLLOWUPS
    }

    fn nav_keys(&self) -> &'static [&'static str] {
        &["9"]
    }

    fn nav_help(&self) -> &'static str {
        "go to the follow-ups"
    }

    fn sections(&self) -> &'static [Section] {
        SECTIONS
    }

    fn contexts(&self, _app: &App) -> Vec<Context> {
        vec![CTX]
    }

    fn draw(&self, frame: &mut Frame, area: Rect, app: &App) {
        crate::ui::followups::render(frame, area, app);
    }

    fn hints(&self, _app: &App) -> &'static [Hint] {
        HINTS
    }

    fn context_stacks(&self) -> Vec<Vec<Context>> {
        vec![vec![CTX]]
    }

    fn handle(&self, app: &mut App, action: Action) -> Handled {
        app.followups_action(action)
    }

    fn tick(&self, app: &mut App, _changes: Changes) {
        app.refresh_followups_if_stale();
    }
}
