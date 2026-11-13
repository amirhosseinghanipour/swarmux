//! Ledger: what happened, what it cost and whether it can be proven.

use ratatui::{Frame, layout::Rect};
use swarmux_application::AppResult;
use swarmux_domain::keys::{Action, Context, actions};

use super::{Bind, Changes, Handled, Screen, ScreenDef, Section};
use crate::app::{App, InputPurpose, Mode};
use crate::ledger::{Command, LedgerAction};
use crate::ui::input::Input;

pub(super) struct Def;

const CTX: Context = Context::new("ledger");

pub const SEARCH: Action = Action::new("ledger.search");
pub const FILTER: Action = Action::new("ledger.filter");
pub const LABELS: Action = Action::new("ledger.labels");
pub const RERUN: Action = Action::new("ledger.rerun");
pub const FORK: Action = Action::new("ledger.fork");
pub const VERIFY: Action = Action::new("ledger.verify");

const SECTIONS: &[Section] = &[Section {
    context: CTX,
    binds: &[
        Bind {
            action: SEARCH,
            keys: &["/"],
            help: "search the runs",
        },
        Bind {
            action: FILTER,
            keys: &["o"],
            help: "next filter",
        },
        Bind {
            action: LABELS,
            keys: &["l"],
            help: "ask GitHub what became of delivered work",
        },
        Bind {
            action: actions::LIST_PREV,
            keys: &["up"],
            help: "previous run",
        },
        Bind {
            action: actions::LIST_NEXT,
            keys: &["down"],
            help: "next run",
        },
        Bind {
            action: RERUN,
            keys: &["r"],
            help: "run the job again",
        },
        Bind {
            action: FORK,
            keys: &["f"],
            help: "fork the job",
        },
        Bind {
            action: actions::LIST_OPEN,
            keys: &["enter"],
            help: "view the run in focus",
        },
        Bind {
            action: VERIFY,
            keys: &["v"],
            help: "verify the run's evidence bundle",
        },
    ],
}];

impl ScreenDef for Def {
    fn id(&self) -> Screen {
        Screen::Ledger
    }

    fn title(&self) -> &'static str {
        "ledger"
    }

    fn nav_action(&self) -> Action {
        actions::GO_LEDGER
    }

    fn nav_keys(&self) -> &'static [&'static str] {
        &["5"]
    }

    fn nav_help(&self) -> &'static str {
        "go to the ledger"
    }

    fn sections(&self) -> &'static [Section] {
        SECTIONS
    }

    fn contexts(&self, _app: &App) -> Vec<Context> {
        vec![CTX]
    }

    fn draw(&self, frame: &mut Frame, area: Rect, app: &App) {
        crate::ui::ledger::render(frame, area, app);
    }

    fn hints(&self, _app: &App) -> &'static str {
        " / search · o filter · v verify · l labels · r re-run · f fork · enter view · ↑↓ select · q quit"
    }

    fn handle(&self, app: &mut App, action: Action) -> Handled {
        let command = match action {
            SEARCH => Command::Search,
            FILTER => Command::Filter,
            LABELS => Command::Labels,
            actions::LIST_PREV => Command::Prev,
            actions::LIST_NEXT => Command::Next,
            RERUN => Command::Rerun,
            FORK => Command::Fork,
            actions::LIST_OPEN => Command::View,
            VERIFY => Command::Verify,
            _ => return Handled::No,
        };
        let result = run(app, command);
        app.show(result);
        Handled::Yes
    }

    fn enter(&self, app: &mut App) {
        app.refresh_ledger();
    }

    fn tick(&self, app: &mut App, changes: Changes) {
        if changes.jobs || changes.evidence {
            app.refresh_ledger();
        }
    }
}

fn run(app: &mut App, command: Command) -> AppResult<()> {
    match app.ledger.handle(command) {
        LedgerAction::None => {}
        LedgerAction::Moved => app.load_detail(),
        LedgerAction::Filtered => app.refresh_ledger(),
        LedgerAction::Search => {
            let input = Input::with_text(&app.ledger.query);
            app.mode = Mode::Input(InputPurpose::Search, input);
        }
        LedgerAction::Rerun(id) => {
            app.engine.jobs.retry(id)?;
            app.refresh_ledger();
        }
        LedgerAction::Fork(id) => {
            app.engine.jobs.fork(id)?;
            app.refresh_ledger();
        }
        LedgerAction::View(id) => app.view(id)?,
        LedgerAction::Verify(slug) => {
            if let Some(evidence) = app.evidence() {
                evidence.start_verify(&slug)?;
                app.show_toast(format!("verifying {slug}"));
            }
        }
        LedgerAction::Labels => match app.evidence() {
            Some(evidence) => {
                let asked = evidence.request_labels()?;
                app.show_toast(format!("asked GitHub about {asked} branch(es)"));
            }
            None => app.notice = Some("evidence is not set up in this run".into()),
        },
    }
    Ok(())
}
