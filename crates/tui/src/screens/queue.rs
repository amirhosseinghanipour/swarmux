//! Queue: running, queued and recently finished jobs.

use ratatui::{Frame, layout::Rect};
use swarmux_domain::keys::{Action, Context, actions};

use super::{Bind, Handled, Hint, Screen, ScreenDef, Section};
use crate::app::App;
use crate::keys::HELP_TOGGLE;
use crate::queue::{Command, QueueAction};

pub(super) struct Def;

const CTX: Context = Context::new("queue");

pub const CANCEL: Action = Action::new("queue.cancel");
pub const RETRY: Action = Action::new("queue.retry");
pub const PRIORITY_UP: Action = Action::new("queue.priority_up");
pub const PRIORITY_DOWN: Action = Action::new("queue.priority_down");

const SECTIONS: &[Section] = &[Section {
    context: CTX,
    binds: &[
        Bind {
            action: actions::LIST_PREV,
            keys: &["up"],
            help: "previous job",
        },
        Bind {
            action: actions::LIST_NEXT,
            keys: &["down"],
            help: "next job",
        },
        Bind {
            action: CANCEL,
            keys: &["c"],
            help: "cancel the job",
        },
        Bind {
            action: RETRY,
            keys: &["r"],
            help: "run the job again",
        },
        Bind {
            action: PRIORITY_UP,
            keys: &["+", "="],
            help: "raise the job's priority",
        },
        Bind {
            action: PRIORITY_DOWN,
            keys: &["-"],
            help: "lower the job's priority",
        },
        Bind {
            action: actions::LIST_OPEN,
            keys: &["enter"],
            help: "view the job in focus",
        },
    ],
}];

/// The footer.
const HINTS: &[Hint] = &[
    Hint::keys(&[CANCEL], "cancel"),
    Hint::keys(&[RETRY], "retry"),
    Hint::keys(&[PRIORITY_UP, PRIORITY_DOWN], "priority"),
    Hint::keys(&[actions::LIST_OPEN], "view"),
    Hint::symbols(&[actions::LIST_PREV, actions::LIST_NEXT], "select"),
    Hint::keys(&[HELP_TOGGLE], "help"),
    Hint::keys(&[actions::QUIT], "quit"),
];

impl ScreenDef for Def {
    fn id(&self) -> Screen {
        Screen::Queue
    }

    fn title(&self) -> &'static str {
        "queue"
    }

    fn nav_action(&self) -> Action {
        actions::GO_QUEUE
    }

    fn nav_keys(&self) -> &'static [&'static str] {
        &["4"]
    }

    fn nav_help(&self) -> &'static str {
        "go to the queue"
    }

    fn sections(&self) -> &'static [Section] {
        SECTIONS
    }

    fn contexts(&self, _app: &App) -> Vec<Context> {
        vec![CTX]
    }

    fn draw(&self, frame: &mut Frame, area: Rect, app: &App) {
        crate::ui::queue::render(frame, area, app);
    }

    fn hints(&self, _app: &App) -> &'static [Hint] {
        HINTS
    }

    fn context_stacks(&self) -> Vec<Vec<Context>> {
        vec![vec![CTX]]
    }

    fn handle(&self, app: &mut App, action: Action) -> Handled {
        let command = match action {
            actions::LIST_PREV => Command::Prev,
            actions::LIST_NEXT => Command::Next,
            CANCEL => Command::Cancel,
            RETRY => Command::Retry,
            PRIORITY_UP => Command::PriorityUp,
            PRIORITY_DOWN => Command::PriorityDown,
            actions::LIST_OPEN => Command::View,
            _ => return Handled::No,
        };
        let rows = app.engine.jobs.queue();
        let result = match app.queue.handle(command, &rows) {
            QueueAction::None => Ok(()),
            QueueAction::Cancel(id) => app.engine.jobs.cancel(id),
            QueueAction::Retry(id) => app.engine.jobs.retry(id).map(|new| {
                app.queue.selected = Some(new);
            }),
            QueueAction::Priority(id, delta) => {
                app.engine.jobs.change_priority(id, delta).map(drop)
            }
            QueueAction::View(id) => app.view(id),
        };
        app.show(result);
        Handled::Yes
    }
}
