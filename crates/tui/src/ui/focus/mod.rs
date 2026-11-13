//! Focus: one agent in detail. A header carrying the same sigil and hue as
//! its card on Mission control, the situation card, the gate lane, a
//! timeline with the user's own events marked, the action stream and the
//! answer, and a reviewer column only when a review is used.

mod answer;
mod cards;
pub mod layout;
pub(crate) mod parts;
mod side;
pub(crate) mod stream;

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_domain::{JobStatus, display_name};

use crate::app::App;
use crate::focus::{ARRIVAL_MS, Focus};
use crate::ui::dashboard::{dim, fit, label};
use crate::ui::look::Icon;
use crate::ui::mission::{age, glyph};

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    // A held call's overlay sits on the bottom rows: keep the newest rows
    // of the stream above it.
    let covered = crate::ui::overlay_rows(app, area.height);
    let area = Rect {
        height: area.height.saturating_sub(covered),
        ..area
    };
    if area.width < layout::MIN_SIZE.0 || area.height < layout::MIN_SIZE.1 {
        let lines = vec![
            Line::from(label(t, "make the window bigger")).centered(),
            Line::from(dim(t, "focus needs 80×24")).centered(),
        ];
        frame.render_widget(Paragraph::new(lines), area);
        return;
    }
    let Some(focus) = Focus::of(app) else {
        let hint = if app.sessions.is_empty() {
            let (go, new) =
                app.goto_keys(crate::app::Screen::Sessions, crate::screens::sessions::NEW);
            format!("press {go}, then {new}, to add a session")
        } else {
            format!(
                "nothing to focus on yet · press {} to send a prompt, or {} on a card",
                crate::hints::key_or_name(app, crate::keys::PROMPT_SEND),
                crate::hints::key_symbol(app, swarmux_domain::keys::actions::LIST_OPEN)
            )
        };
        frame.render_widget(
            Paragraph::new(Line::from(dim(t, fit(&hint, usize::from(area.width))))),
            Rect { height: 1, ..area },
        );
        return;
    };
    let screen = frame.area();
    let plan = layout::plan(
        (screen.width, screen.height),
        area,
        app.look.gaps,
        focus.review.is_some(),
    );
    header(frame, plan.header, app, &focus);
    cards::sat(frame, plan.sat, app, &focus);
    cards::lane(frame, plan.lane, app, &focus);
    cards::timeline(frame, plan.timeline, app, &focus);
    stream::render(frame, plan.stream, app, &focus);
    answer::render(frame, plan.answer, app, &focus);
    if let (Some(rect), Some(review)) = (plan.review, &focus.review) {
        side::review(frame, rect, app, review);
    }
    if let Some(rect) = plan.plan {
        side::plan(frame, rect, app, &focus);
    }
    if let Some(rect) = plan.agents {
        side::agents(frame, rect, app, &focus);
    }
}

/// Two lines: who, in the colours of their card, and what they run on.
fn header(frame: &mut Frame, area: Rect, app: &App, f: &Focus) {
    if area.height == 0 || area.width < 20 {
        return;
    }
    let t = &app.look.theme;
    let w = usize::from(area.width);
    let who = &f.who;
    let hue = t.model(Some(&who.model));

    // The sigil lights up for a moment on arrival, then settles.
    let arriving = app.look.animated()
        && app.frame.saturating_sub(app.focus_state.arrived_at) < app.look.frames(ARRIVAL_MS);
    let sigil_style = if arriving {
        Style::new().fg(t.on_accent).bg(hue)
    } else {
        Style::new().fg(hue)
    };
    let (mark, word, color) = match (who.status, who.waiting) {
        (JobStatus::Running, true) => (glyph(app, Icon::StateWait), "waiting for you", t.warn),
        (JobStatus::Running, false) => (
            if app.look.animated() {
                app.look.spinner().frame(app.frame, 2)
            } else {
                glyph(app, Icon::StateRun)
            },
            "running",
            t.ok,
        ),
        (JobStatus::Queued, _) => (glyph(app, Icon::StateWait), "queued", t.muted),
        (JobStatus::Done, _) => (glyph(app, Icon::StateDone), "done", t.muted),
        (JobStatus::Failed | JobStatus::Interrupted, _) => {
            (glyph(app, Icon::StateErr), who.status.as_str(), t.error)
        }
        (JobStatus::Cancelled, _) => (glyph(app, Icon::StateIdle), "cancelled", t.dim),
    };
    let head = vec![
        Span::styled(
            format!(" {} ", who.sigil),
            sigil_style.add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{} ", fit(&who.name, 24)),
            Style::new().fg(t.title).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{mark} {word}  "), Style::new().fg(color)),
    ];
    let tail = {
        let mut parts = Vec::new();
        if let Some(ms) = who.elapsed_ms {
            parts.push(age(ms));
        }
        parts.push(format!("${:.2}", who.cost_usd));
        Line::from(Span::styled(
            format!("{} ", parts.join(" · ")),
            Style::new().fg(t.muted),
        ))
    };
    let used = Line::from(head.clone()).width();
    let goal_w = w.saturating_sub(used + tail.width() + 1);
    let mut first = head;
    first.push(Span::styled(
        fit(&who.goal, goal_w),
        Style::new().fg(t.text),
    ));
    frame.render_widget(Line::from(first), Rect { height: 1, ..area });
    frame.render_widget(tail.right_aligned(), Rect { height: 1, ..area });

    if area.height < 2 {
        return;
    }
    let mut badge = vec![display_name(&who.model)];
    if !who.permission_mode.is_empty() {
        badge.push(who.permission_mode.clone());
    }
    if who.lean {
        badge.push("lean".into());
    }
    if let Some(branch) = &f.checkpoints.branch {
        badge.push(format!("{} {branch}", glyph(app, Icon::Git)));
    }
    // The worktree is where the job's files really are.
    match (&f.checkpoints.worktree, &who.cwd) {
        (Some(worktree), _) => badge.push(worktree.clone()),
        (None, Some(cwd)) => badge.push(cwd.display().to_string()),
        (None, None) => {}
    }
    let cache = who.cache.map_or_else(
        || "cache —".to_string(),
        |c| format!("cache {:.0}%", c * 100.0),
    );
    let ctx = who.context.map_or_else(
        || cache.clone(),
        |c| format!("ctx {:.0}% · {cache}", c * 100.0),
    );
    let right = Line::from(Span::styled(format!("{ctx} "), Style::new().fg(t.muted)));
    let room = w.saturating_sub(right.width() + 2);
    frame.render_widget(
        Line::from(vec![
            Span::raw("   "),
            Span::styled(
                fit(&badge.join(" · "), room.saturating_sub(2)),
                Style::new().fg(t.label),
            ),
        ]),
        Rect {
            y: area.y + 1,
            height: 1,
            ..area
        },
    );
    frame.render_widget(
        right.right_aligned(),
        Rect {
            y: area.y + 1,
            height: 1,
            ..area
        },
    );
}

#[cfg(test)]
mod tests;
