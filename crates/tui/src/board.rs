//! The Board on screen: what the agents of each mission wrote to each
//! other, grouped by kind, and the keys that answer their questions.
//!
//! Everything shown here came from an agent, so it is shown the way the
//! digest shows it: through [`BoardEntry::line`], which cleans it and
//! quotes it with its provenance.

use std::rc::Rc;

use swarmux_application::{AppResult, BoardInbox, BoardService};
use swarmux_domain::AttentionKind;
use swarmux_domain::board::{BoardEntry, BoardKind};

use crate::app::{App, InputPurpose, Mode, Screen};
use crate::mission::Pane;
use crate::screens::Handled;
use crate::ui::input::Input;

/// Entries the screen lists.
const SHOWN: usize = 300;

/// The Board screen's selection.
#[derive(Default)]
pub struct BoardScreen {
    /// The selected entry, by place among the entries.
    pub selected: usize,
    /// The question the answer being typed is for.
    pub answering: Option<i64>,
}

/// One row of the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoardRow {
    /// A kind and how many entries of it follow.
    Header(&'static str, usize),
    Entry(Box<BoardEntry>),
}

/// The groups in the order they are listed: what needs a person first.
const ORDER: [(&str, BoardKind); 6] = [
    ("open questions", BoardKind::Question),
    ("claims", BoardKind::Claim),
    ("decisions", BoardKind::Decision),
    ("facts", BoardKind::Fact),
    ("follow-ups", BoardKind::Followup),
    ("reports", BoardKind::Report),
];

impl App {
    /// Lets agents' board calls be answered, the questions they ask reach
    /// the inbox and the Board screen show what they wrote.
    pub fn with_board(mut self, board: Option<(Rc<BoardService>, Rc<dyn BoardInbox>)>) -> App {
        self.set_board(board);
        self
    }

    pub(crate) fn set_board(&mut self, board: Option<(Rc<BoardService>, Rc<dyn BoardInbox>)>) {
        self.engine.board = board;
    }

    /// Whether the Board is on.
    pub fn board_on(&self) -> bool {
        self.engine.board.is_some()
    }

    /// The questions agents asked, for the inbox.
    pub(crate) fn board_items(&self) -> Vec<swarmux_domain::AttentionItem> {
        self.engine
            .board
            .as_ref()
            .map(|(service, _)| service.question_items())
            .unwrap_or_default()
    }

    /// The screen's rows: each kind under a header, newest first.
    pub fn board_rows(&self) -> Vec<BoardRow> {
        let Some((service, _)) = &self.engine.board else {
            return Vec::new();
        };
        let mut all = service.board().entries();
        all.reverse();
        all.truncate(SHOWN);
        let mut rows = Vec::new();
        for (name, kind) in ORDER {
            let group: Vec<&BoardEntry> = all
                .iter()
                .filter(|e| e.kind == kind)
                .filter(|e| {
                    // Open questions have their own group; answered ones
                    // and closed claims follow the live ones.
                    (kind != BoardKind::Question) || e.is_open_question() || e.closed
                })
                .collect();
            if group.is_empty() {
                continue;
            }
            // Live claims and open questions first, then the rest.
            let (live, rest): (Vec<_>, Vec<_>) = group
                .into_iter()
                .partition(|e| e.is_live_claim() || e.is_open_question());
            let count = live.len() + rest.len();
            let name = if kind == BoardKind::Question && live.is_empty() {
                "questions"
            } else {
                name
            };
            rows.push(BoardRow::Header(name, count));
            rows.extend(
                live.into_iter()
                    .chain(rest)
                    .map(|e| BoardRow::Entry(Box::new(e.clone()))),
            );
        }
        rows
    }

    /// The entries of [`App::board_rows`], without the headers.
    pub fn board_entries(&self) -> Vec<BoardEntry> {
        self.board_rows()
            .into_iter()
            .filter_map(|r| match r {
                BoardRow::Entry(e) => Some(*e),
                BoardRow::Header(..) => None,
            })
            .collect()
    }

    /// The question the keys answer: the selected entry of the Board
    /// screen, or the selected question of the inbox.
    fn board_target(&self) -> Option<i64> {
        match self.screen {
            Screen::Board => self
                .board_entries()
                .get(self.board_screen.selected)
                .filter(|e| e.is_open_question())
                .map(|e| e.id),
            Screen::Mission if self.mission.pane == Pane::Inbox => {
                let item = self
                    .mission
                    .selected_item()
                    .filter(|i| i.kind == AttentionKind::BoardQuestion)?;
                item.id.as_str().strip_prefix("ask:")?.parse().ok()
            }
            _ => None,
        }
    }

    /// Answer the selected question: on the Board screen, or in the inbox
    /// of Mission control. Does nothing when no question is selected.
    pub(crate) fn board_answer(&mut self) -> Handled {
        let Some(id) = self.board_target() else {
            return Handled::No;
        };
        self.board_screen.answering = Some(id);
        self.mode = Mode::Input(InputPurpose::BoardAnswer, Input::default());
        Handled::Yes
    }

    /// Close the selected question without an answer (the Board screen).
    pub(crate) fn board_dismiss(&mut self) -> Handled {
        if self.screen != Screen::Board {
            return Handled::No;
        }
        let Some(id) = self.board_target() else {
            return Handled::No;
        };
        let result = self.dismiss_question(id);
        self.show(result);
        Handled::Yes
    }

    fn dismiss_question(&mut self, id: i64) -> AppResult<()> {
        if let Some((service, _)) = &self.engine.board {
            service.dismiss(id)?;
            self.notice = Some(format!("question #{id} closed without an answer"));
        }
        Ok(())
    }

    /// The text typed for a question: the agents see it with `read`.
    pub(crate) fn submit_board_answer(&mut self, text: &str) -> AppResult<()> {
        let Some(id) = self.board_screen.answering.take() else {
            return Ok(());
        };
        if let Some((service, _)) = &self.engine.board {
            service.answer(id, text)?;
            self.notice = Some(format!("answered question #{id}: agents see it with read"));
        }
        Ok(())
    }

    /// The question's text, for the prompt line of the answer.
    pub fn answering_question(&self) -> Option<String> {
        let id = self.board_screen.answering?;
        self.board_entries()
            .into_iter()
            .find(|e| e.id == id)
            .map(|e| swarmux_domain::board::clean(&e.text, 80))
    }
}

#[cfg(test)]
mod tests;
