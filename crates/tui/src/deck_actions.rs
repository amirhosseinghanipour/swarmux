//! What the Deck's keys do: move through the cards and the sessions, follow
//! an edge, and open what is selected in Work. Opening goes through the Work
//! screen's own selection, with the Deck on the back stack, so `Esc`
//! returns to it.

use swarmux_application::console::ConsoleSnapshot;
use swarmux_domain::JobId;
use swarmux_domain::console::{DeckConfig, DeckLayout, Panel, resolve_deck};

use crate::app::{App, Screen};
use crate::deck::{Focus, Plan, Toward};
use crate::ui::deck::foldable;

impl App {
    /// What `[deck]` asks of the Deck, checked: panels, direction and the
    /// sessions strip's length, and what could not be used.
    pub(crate) fn deck_layout(&self) -> &DeckLayout {
        &self.deck_layout
    }

    /// Uses the user's `[deck]`. What can not be used is left out, and the
    /// Deck says so above its panels (`swarmux deck lint` says what).
    pub fn with_deck_config(mut self, config: &DeckConfig) -> App {
        self.deck_layout = resolve_deck(config);
        self
    }

    /// `j` and `k`: the next card in reading order, or the next session
    /// while the strip has the keys.
    pub(crate) fn deck_step(&mut self, delta: isize) {
        if self.deck_screen.focus == Focus::Sessions {
            let last = self.wall().len().saturating_sub(1);
            let at = &mut self.deck_screen.session_at;
            *at = at.saturating_add_signed(delta).min(last);
            return;
        }
        if let Some(snap) = self.console.current() {
            let plan = Plan::of(snap);
            self.deck_screen.step(&plan, delta);
        }
    }

    /// `h` and `l`: along an edge. False when the card has none that way.
    pub(crate) fn deck_toward(&mut self, side: Toward) -> bool {
        if self.deck_screen.focus == Focus::Sessions {
            return false;
        }
        let Some(snap) = self.console.current() else {
            return false;
        };
        let plan = Plan::of(snap);
        self.deck_screen.toward(&plan, side)
    }

    /// `s`: the sessions strip takes the keys, or gives them back. At a
    /// size that folds panels it also takes the graph's place.
    pub(crate) fn deck_sessions(&mut self) {
        if !self.deck_layout.shows(Panel::Sessions) {
            self.notice = Some("sessions is not on the deck: add it to [deck] panels".into());
            return;
        }
        if self.deck_screen.focus == Focus::Sessions {
            self.deck_screen.focus = Focus::Graph;
            self.deck_screen.folded = None;
        } else {
            self.deck_screen.focus = Focus::Sessions;
            self.deck_screen.folded = Some(Panel::Sessions);
        }
    }

    /// `Tab`: at a size that folds panels, the next folded one in place of
    /// the graph; after the last, the graph again.
    pub(crate) fn deck_more(&mut self) {
        let cycle = foldable(&self.deck_layout);
        let next = match self.deck_screen.folded {
            None => cycle.first().copied(),
            Some(now) => cycle
                .iter()
                .position(|p| *p == now)
                .and_then(|i| cycle.get(i + 1).copied()),
        };
        self.deck_screen.folded = next;
        self.deck_screen.focus = if next == Some(Panel::Sessions) {
            Focus::Sessions
        } else {
            Focus::Graph
        };
    }

    /// The outcome and job under the cursor: the selected card's, or the
    /// selected session's.
    fn deck_target(&self, snap: &ConsoleSnapshot) -> Option<(String, Option<i64>)> {
        if self.deck_screen.focus == Focus::Sessions {
            let tiles = self.wall();
            let job = tiles
                .get(
                    self.deck_screen
                        .session_at
                        .min(tiles.len().saturating_sub(1)),
                )?
                .job?
                .0;
            let outcome = snap
                .deck
                .nodes
                .iter()
                .find(|n| n.job == Some(job))
                .map(|n| n.outcome.clone())
                .or_else(|| {
                    snap.groups
                        .iter()
                        .flat_map(|g| &g.outcomes)
                        .find(|o| o.jobs.iter().any(|j| j.id == job))
                        .map(|o| o.id.clone())
                })?;
            return Some((outcome, Some(job)));
        }
        let plan = Plan::of(snap);
        let node = &snap.deck.nodes[self.deck_screen.current(&plan)?];
        Some((node.outcome.clone(), node.job))
    }

    /// `Enter`: the outcome in Work, with the Deck behind it on the back
    /// stack. An outcome that is on no queue (nothing to decide on it) has
    /// no place in Work: its job opens in Focus instead.
    pub(crate) fn deck_open(&mut self) {
        let Some(snap) = self.console.current() else {
            return;
        };
        let Some((outcome, job)) = self.deck_target(snap) else {
            self.notice = Some("nothing is selected".into());
            return;
        };
        if self.work_screen.select(snap, &outcome) {
            self.navigate(Screen::Work);
        } else if let Some(job) = job {
            let result = self.view(JobId(job));
            self.show(result);
        } else {
            self.notice = Some("this card is on no queue and has no job to open".into());
        }
    }
}
