//! The dashboard: the lead, its subagents, the guard and the reviewer of
//! the current job, placed by the active layout. Panels only render what
//! the domain and application already worked out.

mod guard;
pub mod header;
mod lead;
mod log;
mod parts;
mod reviewer;
pub mod status;
mod subagents;
mod timeline;
mod who;

#[cfg(test)]
pub(crate) mod tests;

use ratatui::{Frame, layout::Rect, text::Line, widgets::Paragraph};
use swarmux_domain::JobProgress;

pub use parts::{buckets, dim, fit, label, text};

use crate::app::App;
use crate::theme::Theme;
use crate::ui::grid::{Panel, PanelId, ViewModel, layout};
use crate::ui::look::Icon;

/// What the panels show, gathered once per frame.
pub struct View<'a> {
    /// The job followed (or the latest one).
    pub job: Option<&'a JobProgress>,
    /// Its review run, once queued.
    pub review: Option<&'a JobProgress>,
    /// Where its timeline ends, in ticks.
    pub end: u32,
}

impl<'a> View<'a> {
    pub fn of(app: &'a App) -> View<'a> {
        let job = if app.sessions.is_empty() {
            None
        } else {
            app.current_job()
        };
        View {
            job,
            review: job.and(app.review_run()),
            end: app.timeline_end(),
        }
    }
}

/// The panel behind `id`, if it is one of the dashboard's.
pub fn panel(id: PanelId) -> Option<&'static dyn Panel> {
    static LEGEND: header::Strip = header::Strip(PanelId::Legend);
    static COMMAND: header::Strip = header::Strip(PanelId::Command);
    static STEPS: header::Strip = header::Strip(PanelId::Steps);
    static TARGET: header::Strip = header::Strip(PanelId::Target);
    Some(match id {
        PanelId::Legend => &LEGEND,
        PanelId::Command => &COMMAND,
        PanelId::Steps => &STEPS,
        PanelId::Target => &TARGET,
        PanelId::Reviewer => &reviewer::Reviewer,
        PanelId::Lead => &lead::Lead,
        PanelId::Guard => &guard::Guard,
        PanelId::Delegate => &subagents::Delegate,
        PanelId::Subagents => &subagents::Subagents,
        PanelId::Back => &subagents::Back,
        PanelId::Timeline => &timeline::Timeline,
        PanelId::Who => &who::Who,
        PanelId::Log => &log::Log,
        PanelId::Mix => &who::Mix,
        PanelId::SizeHint => &SizeHint,
        _ => return None,
    })
}

/// Renders the active layout in `area`.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let vm = ViewModel::of(app);
    let look = &app.look;
    layout::render(frame, area, look.layout_spec(), &vm, &look.theme, look.gaps);
}

/// Asks for a bigger window when even the smallest arrangement is cramped.
struct SizeHint;

impl Panel for SizeHint {
    fn id(&self) -> PanelId {
        PanelId::SizeHint
    }
    fn title(&self) -> &'static str {
        "window size"
    }
    fn icon(&self) -> Icon {
        Icon::Steps
    }
    fn min_size(&self) -> (u16, u16) {
        (1, 2)
    }
    fn priority(&self) -> u8 {
        0
    }
    fn boxed(&self) -> bool {
        false
    }
    fn render(&self, frame: &mut Frame, area: Rect, vm: &ViewModel, t: &Theme, tick: u64) {
        let size = frame.area();
        let lines = vec![
            self.summary(vm, t, tick),
            Line::from(dim(
                t,
                format!(
                    "the dashboard needs 80×24 (now {}×{})",
                    size.width, size.height
                ),
            )),
        ];
        frame.render_widget(Paragraph::new(lines).centered(), area);
    }
    fn summary(&self, _vm: &ViewModel, t: &Theme, _tick: u64) -> Line<'static> {
        Line::from(label(t, "make the window bigger")).centered()
    }
}
