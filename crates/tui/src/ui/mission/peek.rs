//! The focus peek of the ultrawide tier: a read-only look at the agent the
//! inbox selection points to, without leaving Mission control. `⏎` opens it
//! in Focus.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_domain::display_name;

use super::extra::{followed_job, todo_icon};
use super::{age, glyph, panel, sigil};
use crate::app::App;
use crate::ui::dashboard::{dim, fit, label};
use crate::ui::look::Icon;

pub fn render(frame: &mut Frame, area: Rect, app: &App, now: i64) {
    let t = &app.look.theme;
    let block = panel(
        t,
        glyph(app, Icon::Lead),
        "peek",
        Line::from(dim(t, " ⏎ opens in focus ")),
        t.border,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 16 || inner.height < 3 {
        return;
    }
    let w = usize::from(inner.width);
    let Some(job) = followed_job(app) else {
        frame.render_widget(
            Line::from(dim(t, "select an item to look inside its agent")),
            Rect { height: 1, ..inner },
        );
        return;
    };
    let Some(p) = app.engine.jobs.progress(job) else {
        let text = format!("job #{job} is not from this run: ⏎ replays it");
        frame.render_widget(
            Line::from(dim(t, fit(&text, w))),
            Rect { height: 1, ..inner },
        );
        return;
    };
    let fact = app.mission.facts.iter().find(|f| f.job_id == job);
    let mut out: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(
                sigil(&p.session_name).to_string(),
                Style::new()
                    .fg(t.model(Some(&p.model)))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {} ", fit(&p.session_name, 20)),
                Style::new().fg(t.title).add_modifier(Modifier::BOLD),
            ),
            dim(
                t,
                format!(
                    "#{job} · {} · {}",
                    p.status.as_str(),
                    display_name(&p.model)
                ),
            ),
        ]),
        Line::from(vec![
            dim(t, "› "),
            Span::styled(
                fit(
                    fact.and_then(|f| f.prompt.lines().next())
                        .unwrap_or_default(),
                    w.saturating_sub(2),
                ),
                Style::new().fg(t.text),
            ),
        ]),
        Line::from(dim(
            t,
            format!(
                "${:.2} · {} in · {} out{}",
                p.cost_usd,
                crate::ui::short(p.tokens_in),
                crate::ui::short(p.tokens_out),
                fact.and_then(|f| f.started_at)
                    .map_or(String::new(), |s| format!(" · up {}", age(now - s)))
            ),
        )),
        Line::raw(""),
    ];

    if let Some(act) = app.engine.jobs.job_activity(job)
        && !act.todos.is_empty()
    {
        out.push(Line::from(label(t, "todos")));
        for todo in act.todos.iter().take(8) {
            out.push(Line::from(vec![
                Span::styled(glyph(app, todo_icon(todo.status)), Style::new().fg(t.muted)),
                Span::styled(
                    format!(" {}", fit(&todo.content, w.saturating_sub(2))),
                    Style::new().fg(t.text),
                ),
            ]));
        }
        out.push(Line::raw(""));
    }

    out.push(Line::from(label(t, "recent")));
    let room = usize::from(inner.height).saturating_sub(out.len());
    let log: Vec<_> = p.log.iter().rev().take(room).collect();
    if log.is_empty() {
        out.push(Line::from(dim(t, "nothing logged yet")));
    }
    for line in log.into_iter().rev() {
        out.push(Line::from(Span::styled(
            fit(line, w),
            Style::new().fg(t.muted),
        )));
    }
    frame.render_widget(Paragraph::new(out), inner);
}
