//! A mission's plan on the Missions screen: the task graph as a compact
//! tree with the files each task expects to touch, what is asked of the
//! user, and how the build compared with the plan.

use ratatui::{
    style::Style,
    text::{Line, Span},
};
use swarmux_domain::mission::Mission;
use swarmux_domain::plan::{Plan, PlanStatus};
use swarmux_domain::taskgraph::{GraphRun, TaskState, width};

use super::{g, heading};
use crate::app::App;
use crate::ui::dashboard::{dim, fit, label};

/// Longest list of files shown beside a task, in characters.
const FILES_MAX: usize = 60;
/// Changed files shown outside the plan.
const OUTSIDE_SHOWN: usize = 5;

/// The plan section of the contract panel; empty when the mission has no
/// plan and does not plan first.
pub(super) fn plan_lines(app: &App, m: &Mission) -> Vec<Line<'static>> {
    let t = &app.look.theme;
    let record = app.mission_plan(&m.slug);
    let first = app.mission_plans_first(&m.slug);
    let status = record.map_or(PlanStatus::Idle, |r| r.status);
    if !first && status == PlanStatus::Idle && record.is_none_or(|r| r.drift.is_none()) {
        return Vec::new();
    }
    let mut out = vec![Line::default(), heading(t, "plan")];
    out.push(Line::from(vec![
        label(t, "plans first "),
        Span::styled(
            if first { "yes" } else { "no" }.to_string(),
            Style::new().fg(t.text),
        ),
        dim(t, "   p toggles"),
    ]));
    let Some(r) = record else {
        return out;
    };
    let (word, color) = match status {
        PlanStatus::Idle => ("no plan yet", t.muted),
        PlanStatus::Planning => ("planning…", t.haiku),
        PlanStatus::Proposed => ("waiting for you", t.warn),
        PlanStatus::Approved => ("approved", t.ok),
        PlanStatus::Failed => ("failed", t.error),
    };
    let mut head = vec![
        label(t, "status      "),
        Span::styled(word.to_string(), Style::new().fg(color)),
    ];
    if r.rounds > 1 {
        head.push(dim(t, format!(" · round {}", r.rounds)));
    }
    if r.cost_usd > 0.0 {
        head.push(dim(t, format!(" · ${:.2}", r.cost_usd)));
    }
    out.push(Line::from(head));
    if status == PlanStatus::Failed && !r.note.is_empty() {
        out.push(Line::from(Span::styled(
            r.note.clone(),
            Style::new().fg(t.error),
        )));
    }
    if let Some(plan) = &r.plan {
        if !plan.summary.is_empty() {
            out.push(Line::from(Span::styled(
                plan.summary.clone(),
                Style::new().fg(t.text),
            )));
        }
        for row in plan.tree() {
            let branch = if row.depth == 0 {
                String::new()
            } else {
                format!("{}{} ", "  ".repeat(row.depth - 1), g(app, "└", "`"))
            };
            let mut spans = vec![
                Span::raw(format!("  {branch}")),
                Span::styled(format!("{} ", row.id), Style::new().fg(t.title)),
                Span::styled(row.title.clone(), Style::new().fg(t.text)),
            ];
            if let Some(task) = r.graph.as_ref().and_then(|run| run.task(&row.id)) {
                spans.push(task_chip(app, task.state, task.reinvoked));
            }
            if row.after.len() > 1 {
                spans.push(dim(t, format!(" ← {}", row.after.join(", "))));
            }
            out.push(Line::from(spans));
            let indent = "  ".repeat(row.depth + 1);
            out.push(Line::from(dim(
                t,
                format!("  {indent}{}", fit(&row.files.join(", "), FILES_MAX)),
            )));
        }
        if let Some(run) = &r.graph {
            out.extend(graph_lines(app, plan, run));
        }
        if status == PlanStatus::Proposed {
            let key = |k: &str, what: &str| {
                [
                    Span::styled(
                        k.to_string(),
                        Style::new()
                            .fg(t.title)
                            .add_modifier(ratatui::style::Modifier::BOLD),
                    ),
                    dim(t, format!(" {what}")),
                ]
            };
            let mut keys = Vec::new();
            keys.extend(key("y", "approve and build · "));
            keys.extend(key("n", "change (with a note) · "));
            keys.extend(key("c", "cancel"));
            out.push(Line::default());
            out.push(Line::from(keys));
        }
    }
    if let Some(d) = &r.drift {
        out.push(Line::default());
        out.push(heading(t, "against the plan"));
        let color = if d.drifted() { t.error } else { t.ok };
        let mark = if d.drifted() {
            g(app, "⚠", "!")
        } else {
            g(app, "✓", "+")
        };
        out.push(Line::from(Span::styled(
            format!("{mark} {}", d.summary()),
            Style::new().fg(color),
        )));
        for f in d.outside.iter().take(OUTSIDE_SHOWN) {
            out.push(Line::from(dim(t, format!("    outside: {f}"))));
        }
        if d.outside.len() > OUTSIDE_SHOWN {
            out.push(Line::from(dim(
                t,
                format!("    … {} more", d.outside.len() - OUTSIDE_SHOWN),
            )));
        }
        if !d.untouched.is_empty() {
            out.push(Line::from(dim(
                t,
                format!("    no change for: {}", d.untouched.join(", ")),
            )));
        }
    }
    out
}

/// A short tag for the card's footer: `plan waiting`, `drift`, …
pub(super) fn card_tag(app: &App, m: &Mission) -> Option<(String, bool)> {
    let r = app.mission_plan(&m.slug)?;
    if let Some(d) = r.drift.as_ref().filter(|d| d.drifted()) {
        return Some((format!(" · drift {:.0}%", d.score() * 100.0), true));
    }
    if let Some(run) = &r.graph
        && m.state.in_flight()
    {
        return Some((
            format!(" · graph {}/{} merged", run.merged.len(), run.tasks.len()),
            run.blocked(),
        ));
    }
    match r.status {
        PlanStatus::Planning => Some((" · planning…".into(), false)),
        PlanStatus::Proposed => Some((" · plan waiting: y/n/c".into(), true)),
        PlanStatus::Failed => Some((" · plan failed".into(), true)),
        PlanStatus::Approved => Some((" · plan approved".into(), false)),
        PlanStatus::Idle => None,
    }
}

/// A task's state as a mark and a word, never by colour alone.
fn task_chip(app: &App, state: TaskState, reinvoked: bool) -> Span<'static> {
    let t = &app.look.theme;
    let (mark, color) = match state {
        TaskState::Waiting => (g(app, "·", "."), t.muted),
        TaskState::Running => (g(app, "▸", ">"), t.builder),
        TaskState::Gate => (g(app, "◇", "?"), t.haiku),
        TaskState::Merging => (g(app, "⇢", "~"), t.haiku),
        TaskState::Merged => (g(app, "✓", "+"), t.ok),
        TaskState::Conflict => (g(app, "↻", "@"), t.warn),
        TaskState::Failed => (g(app, "✗", "x"), t.error),
    };
    let again = if reinvoked && state != TaskState::Merged {
        " ×1"
    } else {
        ""
    };
    Span::styled(
        format!("  {mark} {}{again}", state.as_str()),
        Style::new().fg(color),
    )
}

/// The graph's summary, its merge queue, the claims held and what needs
/// the user.
fn graph_lines(app: &App, plan: &Plan, run: &GraphRun) -> Vec<Line<'static>> {
    let t = &app.look.theme;
    let mut out = vec![Line::default(), heading(t, "task graph")];
    let counts: Vec<String> = run
        .counts()
        .into_iter()
        .map(|(s, n)| format!("{n} {}", s.as_str()))
        .collect();
    out.push(Line::from(vec![
        label(t, "topology    "),
        Span::styled(
            format!("{} ×{}", run.topology.as_str(), width(plan)),
            Style::new().fg(t.text),
        ),
        dim(t, format!(" · {}", counts.join(" · "))),
    ]));
    out.push(Line::from(vec![
        label(t, "integration "),
        dim(t, fit(&run.integration, FILES_MAX)),
    ]));
    let queue = run.queue(plan);
    out.push(Line::from(vec![
        label(t, "merge queue "),
        if queue.is_empty() {
            dim(t, format!("empty · {} merged", run.merged.len()))
        } else {
            Span::styled(
                queue.join(&format!(" {} ", g(app, "→", ">"))),
                Style::new().fg(t.text),
            )
        },
    ]));
    let held: Vec<String> = plan
        .tasks
        .iter()
        .filter(|p| run.task(&p.id).is_some_and(|r| r.state.holds_claim()))
        .map(|p| format!("{} {}", p.id, p.files.join(", ")))
        .collect();
    if !held.is_empty() {
        out.push(Line::from(vec![
            label(t, "claims      "),
            dim(t, fit(&held.join(" · "), FILES_MAX * 2)),
        ]));
    }
    if run.denials > 0 {
        let s = if run.denials == 1 { "" } else { "s" };
        out.push(Line::from(vec![
            label(t, "scope       "),
            Span::styled(
                format!("{} write{s} outside a claim refused", run.denials),
                Style::new().fg(t.warn),
            ),
        ]));
    }
    for task in run.tasks.iter().filter(|x| x.state == TaskState::Failed) {
        out.push(Line::from(Span::styled(
            format!("{} {}: {}", g(app, "✗", "x"), task.id, task.note),
            Style::new().fg(t.error),
        )));
    }
    out
}
