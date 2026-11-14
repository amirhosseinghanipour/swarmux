//! The Return brief's state and what its keys do: the window it is about
//! (from when the user was last here to when they came back), the
//! ledger's digest of it, the view built from that and the console
//! snapshot, and the jumps out of it.
//!
//! The window is fixed the first time the brief is built, so it keeps
//! saying what happened while the user was away however long they then
//! stay. What stands now (the decisions, what is ready to land) is read
//! from the live snapshot every time.

use swarmux_application::console::{BriefInput, BriefView, build_brief, read_digest};
use swarmux_domain::Millis;
use swarmux_domain::console::{BriefConfig, BriefDigest, BriefSettings, EscGoesTo, resolve_brief};

use crate::app::{App, Screen};

/// The Brief screen's state.
#[derive(Default)]
pub struct BriefScreen {
    pub settings: BriefSettings,
    /// Fixed at the first build: `None` when there is no history.
    window: Option<(Millis, Millis)>,
    built: bool,
    digest: Option<BriefDigest>,
    /// The view the screen draws, rebuilt while the screen shows.
    pub view: Option<BriefView>,
    /// At a small size: the folded sections in place of the main ones.
    pub more: bool,
}

impl BriefScreen {
    pub fn window(&self) -> Option<(Millis, Millis)> {
        self.window
    }
}

impl App {
    /// Uses the user's `[brief]`. A mistake in it never stops swarmux: the
    /// default applies for what could not be used, and one line says so.
    /// When the user has been away longer than `away_after_minutes`, the
    /// app starts on the brief (unless `show_on_attach` is false).
    pub fn with_brief_config(mut self, config: &BriefConfig) -> App {
        let settings = resolve_brief(config);
        if let Some(first) = settings.problems.first() {
            let more = settings.problems.len() - 1;
            let note = format!(
                "{first}{} · run `swarmux return-brief lint`",
                if more > 0 {
                    format!(" (+{more} more)")
                } else {
                    String::new()
                }
            );
            self.notice = Some(match self.notice.take() {
                Some(old) => format!("{old}; {note}"),
                None => note,
            });
        }
        let gap = self.away_since.map(|at| self.now() - at);
        if settings.show_on_attach && gap.is_some_and(|g| g >= settings.away_after_ms()) {
            self.screen = Screen::Brief;
        }
        self.brief.settings = settings;
        self
    }

    /// Builds the brief from the snapshot as it stands now. The window and
    /// the ledger's digest of it are read the first time only.
    pub(crate) fn brief_refresh(&mut self) {
        let now = self.now();
        if !self.brief.built {
            self.brief.built = true;
            self.brief.window = self.away_since.map(|from| (from, now.max(from)));
            if let (Some((from, to)), Some(ledger)) = (self.brief.window, &self.engine.ledger) {
                match read_digest(ledger.ledger(), from, to) {
                    Ok(d) => self.brief.digest = Some(d),
                    Err(e) => {
                        self.error = Some(format!("the brief could not read the ledger: {e}"))
                    }
                }
            }
        }
        let window = self.brief.window;
        let digest = self.brief.digest.take();
        let view = {
            let snap = self.console_snapshot();
            build_brief(
                snap,
                &BriefInput {
                    now,
                    window,
                    digest: digest.as_ref(),
                },
            )
        };
        self.brief.digest = digest;
        self.brief.view = Some(view);
    }

    /// `Enter`: the first decision in Work, with the brief behind it on
    /// the back stack.
    pub(crate) fn brief_open(&mut self) {
        let first = self
            .brief
            .view
            .as_ref()
            .and_then(|v| v.decisions.first())
            .map(|d| d.id.clone());
        let Some(id) = first else {
            self.notice = Some("no decision is waiting".into());
            return;
        };
        let Some(snap) = self.console.current() else {
            return;
        };
        if self.work_screen.select(snap, &id) {
            self.navigate(Screen::Work);
        } else {
            self.notice = Some("that decision is no longer on the queue".into());
        }
    }

    /// `Esc`: past the brief, to the screen `[brief] esc_goes_to` names.
    pub(crate) fn brief_skip(&mut self) {
        let to = match self.brief.settings.esc_goes_to {
            EscGoesTo::Work => Screen::Work,
            EscGoesTo::Deck => Screen::Deck,
        };
        self.skip_to(to);
    }

    /// `Tab`: at a small size, the folded sections or the main ones.
    pub(crate) fn brief_more(&mut self) {
        self.brief.more = !self.brief.more;
    }
}
