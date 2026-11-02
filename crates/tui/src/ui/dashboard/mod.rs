//! The dashboard: the lead, its subagents, the guard and the reviewer of
//! the current job, laid out to fit the window. Panels only render what the
//! domain and application already worked out.

mod guard;
pub mod header;
mod layout;
mod lead;
mod log;
mod reviewer;
pub mod status;
mod subagents;
mod timeline;
mod who;
mod widgets;

#[cfg(test)]
mod tests;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::Line,
    widgets::Paragraph,
};
use swarmux_domain::JobProgress;

pub use layout::Tier;

use crate::app::App;
use widgets::{dim, label};

/// What the panels show, gathered once per frame.
pub struct View<'a> {
    /// The job followed (or the latest one).
    pub job: Option<&'a JobProgress>,
    /// Its review run, once queued.
    pub review: Option<&'a JobProgress>,
    /// Where its timeline ends, in ticks.
    pub end: u32,
    /// Frame counter for the spinners.
    pub frame: u64,
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
            frame: app.frame,
        }
    }
}

/// The tallest the main row (reviewer beside lead, guard and subagents) gets.
const MAIN_H: u16 = 25;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let size = frame.area();
    let view = View::of(app);
    match Tier::for_size(size.width, size.height) {
        Tier::TooSmall => too_small(frame, area, size),
        Tier::Minimal => minimal(frame, area, app, &view),
        Tier::Compact => compact(frame, area, app, &view),
        Tier::NoWho => full(frame, area, app, &view, false),
        Tier::Full => full(frame, area, app, &view, true),
    }
}

fn too_small(frame: &mut Frame, area: Rect, size: Rect) {
    let lines = vec![
        Line::from(label("make the window bigger")),
        Line::from(dim(format!(
            "the dashboard needs 80×24 (now {}×{})",
            size.width, size.height
        ))),
    ];
    let top = area.y + area.height.saturating_sub(2) / 2;
    let rect = Rect {
        y: top,
        height: area.height.min(2),
        ..area
    };
    frame.render_widget(Paragraph::new(lines).centered(), rect);
}

/// The header rows every tier but the smallest has.
fn heading(frame: &mut Frame, rows: [Rect; 4], app: &App, view: &View) {
    let [legend, command, steps, target] = rows;
    frame.render_widget(header::legend(view), legend);
    header::command(frame, command, view);
    header::steps(frame, steps, view, crate::ui::hints(app));
    header::target(frame, target, app);
}

fn full(frame: &mut Frame, area: Rect, app: &App, view: &View, with_who: bool) {
    let main_h = MAIN_H.min(area.height.saturating_sub(4 + 9));
    let [l, c, s, t, main, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(main_h),
        Constraint::Fill(1),
    ])
    .areas(area);
    heading(frame, [l, c, s, t], app, view);

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(28), Constraint::Fill(1)]).areas(main);
    reviewer::panel(frame, left, app, view);
    let guard_h = if main_h >= MAIN_H { 8 } else { 6 };
    let [lead_a, guard_a, delegate_a, cards_a, back_a] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length(guard_h),
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(4),
    ])
    .areas(right);
    lead::panel_box(frame, lead_a, app, view);
    guard::panel_box(frame, guard_a, app);
    subagents::delegate(frame, delegate_a, view);
    subagents::cards(frame, cards_a, view);
    subagents::back_to_lead(frame, back_a, view);

    if with_who {
        // Timeline lanes (lead, guard, reviewer, subagents) or who-sees-what
        // rows (guard, lead, reviewer, subagents, two caption lines).
        let actors = view.job.map_or(0, |j| {
            2 + j.subagents.len() + usize::from(view.review.is_some())
        }) as u16;
        let top_h = (actors + 4)
            .clamp(7, 11)
            .min(bottom.height.saturating_sub(4));
        let [top, log_a] =
            Layout::vertical([Constraint::Length(top_h), Constraint::Fill(1)]).areas(bottom);
        let [tl, who_a] =
            Layout::horizontal([Constraint::Percentage(60), Constraint::Fill(1)]).areas(top);
        timeline::render(frame, tl, view);
        who::render(frame, who_a, view);
        log::render(frame, log_a, view);
    } else {
        let [tl, log_a] =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Fill(1)]).areas(bottom);
        timeline::render(frame, tl, view);
        log::render(frame, log_a, view);
    }
}

fn list_height(view: &View, max: usize) -> u16 {
    view.job.map_or(1, |j| j.subagents.len().clamp(1, max)) as u16 + 2
}

fn compact(frame: &mut Frame, area: Rect, app: &App, view: &View) {
    let [l, c, s, t, rev, lead_a, guard_a, list_a, back, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(4),
        Constraint::Length(5),
        Constraint::Length(list_height(view, 4)),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    heading(frame, [l, c, s, t], app, view);
    frame.render_widget(reviewer::one_line(app, view), rev);
    lead::panel_box(frame, lead_a, app, view);
    guard::panel_box(frame, guard_a, app);
    subagents::list(frame, list_a, view);
    frame.render_widget(subagents::back_line(view), back);
    let [tl, log_a] =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Fill(1)]).areas(bottom);
    timeline::render(frame, tl, view);
    log::render(frame, log_a, view);
}

fn minimal(frame: &mut Frame, area: Rect, app: &App, view: &View) {
    let [c, s, t, rev, lead_a, guard_a, list_a, back, log_a] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(list_height(view, 3)),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    header::command(frame, c, view);
    header::steps(frame, s, view, "");
    header::target(frame, t, app);
    frame.render_widget(reviewer::one_line(app, view), rev);
    frame.render_widget(lead::one_line(view), lead_a);
    frame.render_widget(guard::one_line(app), guard_a);
    subagents::list(frame, list_a, view);
    frame.render_widget(subagents::back_line(view), back);
    log::render(frame, log_a, view);
}
