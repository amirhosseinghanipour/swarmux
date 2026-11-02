//! The panel grid: every dashboard panel implements [`Panel`], and layouts
//! (see [`layout`]) say where each goes. A panel too small for its full
//! render collapses to a one-line summary; the layout hides the least
//! important ones when even that doesn't fit.

pub mod chrome;
pub mod layout;

use std::cell::RefCell;

use ratatui::{Frame, layout::Rect, text::Line};

use super::anim::{Anim, Timing};
use super::dashboard::View;
use super::look::{Icon, IconSet, Look};
use super::widgets::SpinnerSet;
use crate::app::App;
use crate::telemetry::Telemetry;
use crate::theme::Theme;

/// Every panel a layout can place, by its config id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PanelId {
    // The job, from the dashboard.
    Legend,
    Command,
    Steps,
    Target,
    Reviewer,
    Lead,
    Guard,
    Delegate,
    Subagents,
    Back,
    Timeline,
    Who,
    Log,
    Mix,
    SizeHint,
    // Telemetry panels, empty until their data arrives.
    Usage,
    Burn,
    Tokens,
    Context,
    Agents,
    Todos,
    Tools,
    Files,
    Git,
    Ci,
    System,
    Wall,
    Calendar,
    Env,
}

impl PanelId {
    pub const ALL: [PanelId; 29] = [
        PanelId::Legend,
        PanelId::Command,
        PanelId::Steps,
        PanelId::Target,
        PanelId::Reviewer,
        PanelId::Lead,
        PanelId::Guard,
        PanelId::Delegate,
        PanelId::Subagents,
        PanelId::Back,
        PanelId::Timeline,
        PanelId::Who,
        PanelId::Log,
        PanelId::Mix,
        PanelId::SizeHint,
        PanelId::Usage,
        PanelId::Burn,
        PanelId::Tokens,
        PanelId::Context,
        PanelId::Agents,
        PanelId::Todos,
        PanelId::Tools,
        PanelId::Files,
        PanelId::Git,
        PanelId::Ci,
        PanelId::System,
        PanelId::Wall,
        PanelId::Calendar,
        PanelId::Env,
    ];

    /// The id used in `[[layouts]]`.
    pub fn as_str(self) -> &'static str {
        match self {
            PanelId::Legend => "legend",
            PanelId::Command => "command",
            PanelId::Steps => "steps",
            PanelId::Target => "target",
            PanelId::Reviewer => "reviewer",
            PanelId::Lead => "lead",
            PanelId::Guard => "guard",
            PanelId::Delegate => "delegate",
            PanelId::Subagents => "subagents",
            PanelId::Back => "back",
            PanelId::Timeline => "timeline",
            PanelId::Who => "who",
            PanelId::Log => "log",
            PanelId::Mix => "mix",
            PanelId::SizeHint => "size-hint",
            PanelId::Usage => "usage",
            PanelId::Burn => "burn",
            PanelId::Tokens => "tokens",
            PanelId::Context => "context",
            PanelId::Agents => "agents",
            PanelId::Todos => "todos",
            PanelId::Tools => "tools",
            PanelId::Files => "files",
            PanelId::Git => "git",
            PanelId::Ci => "ci",
            PanelId::System => "system",
            PanelId::Wall => "wall",
            PanelId::Calendar => "calendar",
            PanelId::Env => "env",
        }
    }

    pub fn parse(s: &str) -> Option<PanelId> {
        let s = s.trim().to_ascii_lowercase();
        PanelId::ALL.into_iter().find(|p| p.as_str() == s)
    }

    /// The panel behind this id.
    pub fn panel(self) -> &'static dyn Panel {
        super::dashboard::panel(self).unwrap_or_else(|| super::telemetry::panel(self))
    }
}

/// What every panel reads, gathered once per frame.
pub struct ViewModel<'a> {
    pub app: &'a App,
    /// The job the dashboard follows.
    pub view: View<'a>,
    /// Data from the telemetry feed; empty until it is wired.
    pub data: &'a Telemetry,
    pub icons: IconSet,
    pub spinner: SpinnerSet,
    /// The panel `Tab` focused, if any.
    pub focus: Option<PanelId>,
    anim: &'a RefCell<Anim>,
    timing: Timing,
}

impl<'a> ViewModel<'a> {
    pub fn of(app: &'a App) -> ViewModel<'a> {
        let look: &Look = &app.look;
        ViewModel {
            app,
            view: View::of(app),
            data: &app.telemetry,
            icons: look.icons,
            spinner: look.spinner(),
            focus: app.focus,
            anim: &app.anim,
            timing: Timing {
                enabled: look.animated(),
                tween_frames: look.frames(super::anim::TWEEN_MS),
                pulse_frames: look.frames(super::anim::PULSE_MS),
            },
        }
    }

    pub fn tick(&self) -> u64 {
        self.app.frame
    }

    /// `target` eased in over a few frames (as is in low-power mode).
    pub fn tween(&self, key: &str, target: f64) -> f64 {
        let tick = self.tick();
        match self.anim.try_borrow_mut() {
            Ok(mut a) => a.value(key, target, tick, self.timing),
            Err(_) => target,
        }
    }

    /// How strongly `id` pulses now; a new `fingerprint` starts a pulse.
    pub fn pulse(&self, id: PanelId, fingerprint: u64) -> f64 {
        let tick = self.tick();
        match self.anim.try_borrow_mut() {
            Ok(mut a) => a.pulse(id, fingerprint, tick, self.timing),
            Err(_) => 0.0,
        }
    }

    /// The spinner frame for now.
    pub fn spin(&self) -> &'static str {
        self.spinner.frame(self.tick(), 2)
    }

    pub fn icon(&self, icon: Icon) -> &'static str {
        self.icons.get(icon)
    }
}

/// A dashboard panel.
pub trait Panel: Sync {
    fn id(&self) -> PanelId;
    /// The title on its chip.
    fn title(&self) -> &'static str;
    fn icon(&self) -> Icon;
    /// The smallest area (width, height) its full render needs; anything
    /// smaller gets [`Panel::summary`].
    fn min_size(&self) -> (u16, u16);
    /// Lower stays longer when a layout runs out of room.
    fn priority(&self) -> u8;
    /// One-line panels (the header strips) sit without gaps.
    fn boxed(&self) -> bool {
        true
    }
    /// Rows it would like when its layout row is `auto`.
    fn preferred_height(&self, _vm: &ViewModel) -> Option<u16> {
        None
    }
    fn render(&self, frame: &mut Frame, area: Rect, vm: &ViewModel, theme: &Theme, tick: u64);
    /// The panel as one line, for small windows.
    fn summary(&self, vm: &ViewModel, theme: &Theme, tick: u64) -> Line<'static>;
}

/// Renders `panel` in `area`: in full when it fits, else as one line.
pub fn render_panel(
    frame: &mut Frame,
    area: Rect,
    panel: &dyn Panel,
    vm: &ViewModel,
    theme: &Theme,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let tick = vm.tick();
    let (w, h) = panel.min_size();
    if area.width >= w && area.height >= h {
        panel.render(frame, area, vm, theme, tick);
    } else {
        let line = Rect { height: 1, ..area };
        frame.render_widget(panel.summary(vm, theme, tick), line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_every_id_has_a_panel() {
        for id in PanelId::ALL {
            assert_eq!(PanelId::parse(id.as_str()), Some(id));
            assert_eq!(id.panel().id(), id, "{id:?}");
            let (w, h) = id.panel().min_size();
            assert!(w >= 1 && h >= 1);
        }
        assert_eq!(PanelId::parse(" Lead "), Some(PanelId::Lead));
        assert_eq!(PanelId::parse("nope"), None);
    }
}
