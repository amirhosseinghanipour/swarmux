//! Work: decide and land. The ranked queue of outcomes on the left, the
//! selected one's work product on the right, and the actions it allows.
//! It reads the console snapshot; what it does goes through the same
//! services as the screens it replaces.

use ratatui::{Frame, layout::Rect};
use swarmux_domain::keys::{Action, Context, actions};

use super::{Bind, Changes, Handled, Hint, Screen, ScreenDef, Section};
use crate::app::App;
use crate::keys::{DECIDE_ALWAYS, DECIDE_NO, DECIDE_YES, HELP_TOGGLE};
use crate::work::{Do, Tab};

pub(super) struct Def;

const CTX: Context = Context::new("work");

pub const TAB_PRODUCT: Action = Action::new("work.tab_product");
pub const TAB_STREAM: Action = Action::new("work.tab_stream");
pub const TAB_DIFF: Action = Action::new("work.tab_diff");
pub const TAB_CHECKS: Action = Action::new("work.tab_checks");
pub const SCROLL_UP: Action = Action::new("work.scroll_up");
pub const SCROLL_DOWN: Action = Action::new("work.scroll_down");
pub const LAND: Action = Action::new("work.land");
pub const KILL: Action = Action::new("work.kill");
pub const RETRY: Action = Action::new("work.retry");
pub const RESUME: Action = Action::new("work.resume");
pub const EVIDENCE: Action = Action::new("work.evidence");
pub const SEND_BACK: Action = Action::new("work.send_back");
pub const MESSAGE: Action = Action::new("work.message");
pub const MESSAGE_MODE: Action = Action::new("work.message_mode");

const SECTIONS: &[Section] = &[Section {
    context: CTX,
    binds: &[
        Bind {
            action: actions::LIST_PREV,
            keys: &["up", "k"],
            help: "previous outcome",
        },
        Bind {
            action: actions::LIST_NEXT,
            keys: &["down", "j"],
            help: "next outcome",
        },
        Bind {
            action: TAB_PRODUCT,
            keys: &["1"],
            help: "the work product tab",
        },
        Bind {
            action: TAB_STREAM,
            keys: &["2"],
            help: "the stream tab: the job's actions",
        },
        Bind {
            action: TAB_DIFF,
            keys: &["3"],
            help: "the diff tab: what changed",
        },
        Bind {
            action: TAB_CHECKS,
            keys: &["4"],
            help: "the checks tab: every check and gate stage",
        },
        Bind {
            action: SCROLL_UP,
            keys: &["pageup"],
            help: "scroll the right pane up",
        },
        Bind {
            action: SCROLL_DOWN,
            keys: &["pagedown"],
            help: "scroll the right pane down",
        },
        Bind {
            action: LAND,
            keys: &["l"],
            help: "land the work in its integration branch",
        },
        Bind {
            action: KILL,
            keys: &["x"],
            help: "stop the outcome's running job",
        },
        Bind {
            action: RETRY,
            keys: &["r"],
            help: "queue the job's prompt again",
        },
        Bind {
            action: RESUME,
            keys: &["R"],
            help: "continue an interrupted or failed job",
        },
        Bind {
            action: EVIDENCE,
            keys: &["e"],
            help: "open the evidence in the ledger",
        },
        Bind {
            action: SEND_BACK,
            keys: &["s"],
            help: "send the work back to the agent with a note, or deny a held call with one",
        },
        Bind {
            action: MESSAGE,
            keys: &["m"],
            help: "message the running agent: queued until its turn ends, or stop the turn and carry on with it",
        },
        Bind {
            action: MESSAGE_MODE,
            keys: &["M"],
            help: "switch the message mode between queue and steer",
        },
        Bind {
            action: actions::LIST_OPEN,
            keys: &["enter"],
            help: "open the job in focus",
        },
    ],
}];

/// The footer.
const HINTS: &[Hint] = &[
    Hint::symbols(&[actions::LIST_PREV, actions::LIST_NEXT], "select"),
    Hint::keys(&[DECIDE_YES, DECIDE_NO, DECIDE_ALWAYS], "answer"),
    Hint::keys(&[LAND], "land"),
    Hint::keys(&[SEND_BACK], "send back"),
    Hint::keys(&[MESSAGE], "message"),
    Hint::keys(&[EVIDENCE], "evidence"),
    Hint::keys(&[TAB_PRODUCT, TAB_STREAM, TAB_DIFF, TAB_CHECKS], "tabs"),
    Hint::keys(&[KILL], "kill"),
    Hint::keys(&[RETRY], "retry"),
    Hint::symbols(&[actions::LIST_OPEN], "focus"),
    Hint::keys(&[HELP_TOGGLE], "help"),
    Hint::keys(&[actions::QUIT], "quit"),
];

impl ScreenDef for Def {
    fn id(&self) -> Screen {
        Screen::Work
    }

    fn title(&self) -> &'static str {
        "work"
    }

    fn nav_action(&self) -> Action {
        actions::GO_WORK
    }

    fn nav_keys(&self) -> &'static [&'static str] {
        &["g w"]
    }

    fn nav_help(&self) -> &'static str {
        "go to work: decide and land"
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
        crate::ui::work::render(frame, area, app);
    }

    fn hints(&self, _app: &App) -> &'static [Hint] {
        HINTS
    }

    fn badge(&self, app: &App) -> Option<usize> {
        app.console_current()
            .map(|s| s.header.counts.decide)
            .filter(|n| *n > 0)
    }

    fn handle(&self, app: &mut App, action: Action) -> Handled {
        match action {
            actions::LIST_PREV => app.work_step(-1),
            actions::LIST_NEXT => app.work_step(1),
            TAB_PRODUCT => app.work_tab(Tab::Product),
            TAB_STREAM => app.work_tab(Tab::Stream),
            TAB_DIFF => app.work_tab(Tab::Diff),
            TAB_CHECKS => app.work_tab(Tab::Checks),
            SCROLL_UP => app.work_screen.scroll_by(-10),
            SCROLL_DOWN => app.work_screen.scroll_by(10),
            LAND => app.work_do(Do::Land),
            KILL => app.work_do(Do::Kill),
            RETRY => app.work_do(Do::Retry),
            RESUME => app.work_do(Do::Resume),
            EVIDENCE => app.work_do(Do::Evidence),
            SEND_BACK => app.work_do(Do::SendBack),
            MESSAGE => app.work_do(Do::Message),
            MESSAGE_MODE => app.work_switch_mode(),
            actions::LIST_OPEN => app.work_open_focus(),
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn enter(&self, app: &mut App) {
        app.work_refresh();
    }

    fn tick(&self, app: &mut App, _changes: Changes) {
        app.work_settle();
    }
}
