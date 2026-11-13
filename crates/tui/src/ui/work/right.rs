//! The right pane: the selected outcome's header and tabs, the tab's body,
//! and the actions it allows.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_application::console::{
    ConsoleSnapshot, GroupKind, HoldState, Lifecycle, OutcomeView, PlanStatus,
};
use swarmux_domain::JobId;

use super::util::{Lines, fit, heading, lifecycle_word, span, usd};
use super::{product, queue};
use crate::app::App;
use crate::focus::stream::diff_lines;
use crate::focus::stream::{RowKind, rows};
use crate::ui::focus::parts::diff_style;
use crate::ui::focus::stream::row_line_of;
use crate::ui::mission::age;
use crate::work::{Tab, edge_label, holds_of, relations};

pub(super) fn render(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    snap: &ConsoleSnapshot,
    outcome: Option<&OutcomeView>,
) {
    let t = &app.look.theme;
    let Some(o) = outcome else {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(span("nothing selected", Style::new().fg(t.muted))),
                Line::from(span(
                    "outcomes appear on the left as missions and jobs need you, finish or land",
                    Style::new().fg(t.dim),
                )),
            ]),
            area,
        );
        return;
    };
    let w = usize::from(area.width);
    let header = header(app, snap, o, w);
    let actions = actions(app, &crate::work::rows_for(snap, o), w);
    let header_h = u16::try_from(header.len()).unwrap_or(0).min(area.height);
    let actions_h = u16::try_from(actions.len())
        .unwrap_or(0)
        .min(area.height.saturating_sub(header_h + 3));
    // A row of air under the header, and one above the actions.
    let body_h = area
        .height
        .saturating_sub(header_h + actions_h + 1 + u16::from(actions_h > 0));

    frame.render_widget(
        Paragraph::new(header),
        Rect {
            height: header_h,
            ..area
        },
    );
    let body_area = Rect {
        y: area.y + header_h + 1,
        height: body_h,
        ..area
    };
    let body = body(app, snap, o, w);
    draw_body(frame, body_area, app, body);
    if actions_h > 0 {
        let at = Rect {
            y: area.y + area.height - actions_h,
            height: actions_h,
            ..area
        };
        frame.render_widget(Paragraph::new(actions), at);
    }
}

/// The scrolled window of `lines`. The stream follows its tail; the other
/// tabs start at the top.
fn draw_body(frame: &mut Frame, area: Rect, app: &App, lines: Lines) {
    let height = usize::from(area.height);
    let max = lines.len().saturating_sub(height);
    let state = &app.work_screen;
    state.max_scroll.set(max);
    let scroll = state.scroll.min(max);
    let from = if state.tab == Tab::Stream {
        max - scroll
    } else {
        scroll
    };
    let shown: Lines = lines.into_iter().skip(from).take(height).collect();
    frame.render_widget(Paragraph::new(shown), area);
}

// ---------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------

/// What the state badge says: the state and what it needs.
fn badge(app: &App, snap: &ConsoleSnapshot, o: &OutcomeView) -> (String, ratatui::style::Color) {
    let t = &app.look.theme;
    let holds = holds_of(snap, o);
    if let Some(h) = holds.iter().find(|h| h.state == HoldState::Held) {
        let what = if h.destructive {
            "destructive · typed confirm"
        } else {
            h.rule.as_deref().unwrap_or("needs an answer")
        };
        return (format!("HELD BY GUARD · {what}"), t.warn);
    }
    if o.plan
        .as_ref()
        .is_some_and(|p| p.status == PlanStatus::Proposed)
    {
        return ("PLAN WAITS FOR YOU".into(), t.warn);
    }
    match (o.group, o.lifecycle) {
        (GroupKind::Decide, Lifecycle::Escalated) => ("ESCALATED".into(), t.error),
        (GroupKind::Decide, Lifecycle::Failed | Lifecycle::Interrupted) => {
            (lifecycle_word(o.lifecycle).to_uppercase(), t.error)
        }
        (GroupKind::Decide, _) => ("NEEDS YOU".into(), t.warn),
        (GroupKind::ReadyToLand, _) => ("VERIFIED · not landed yet".into(), t.ok),
        (GroupKind::Running, l) => (lifecycle_word(l).to_uppercase(), t.label),
        (GroupKind::Landed, _) => ("DELIVERED · with proof".into(), t.ok),
    }
}

fn header(app: &App, snap: &ConsoleSnapshot, o: &OutcomeView, w: usize) -> Lines {
    let t = &app.look.theme;
    let now = app.now();
    let (word, hue) = badge(app, snap, o);
    let mut lines: Lines = Vec::new();

    let badge_w = word.chars().count() + 3;
    lines.push(Line::from(vec![
        span(
            fit(&o.title, w.saturating_sub(badge_w)),
            Style::new().fg(t.title).add_modifier(Modifier::BOLD),
        ),
        span("   ", Style::default()),
        span(word, Style::new().fg(hue).add_modifier(Modifier::BOLD)),
    ]));

    let mut facts: Vec<String> = Vec::new();
    if let Some(m) = &o.mission {
        facts.push(format!("mission {m}"));
    }
    let mut repos: Vec<&str> = o.jobs.iter().map(|j| j.repo.as_str()).collect();
    repos.dedup();
    if !repos.is_empty() {
        facts.push(repos.join(", "));
    }
    if let Some(j) = o.jobs.first() {
        facts.push(format!("job #{} {}", j.id, j.model));
    }
    facts.push(usd(o.spend.total_usd));
    facts.push(format!("{} in state", age(now - o.since_at)));
    lines.push(Line::from(span(
        fit(&facts.join(" · "), w),
        Style::new().fg(t.muted),
    )));

    if let Some(c) = &o.contract {
        if !c.done_when.is_empty() {
            lines.push(Line::from(vec![
                span("done when ", Style::new().fg(t.dim)),
                span(
                    fit(&c.done_when.join(" · "), w.saturating_sub(10)),
                    Style::new().fg(t.text),
                ),
            ]));
        }
        if let Some(note) = &c.note {
            lines.push(Line::from(vec![
                span("note ", Style::new().fg(t.dim)),
                span(fit(note, w.saturating_sub(5)), Style::new().fg(t.warn)),
            ]));
        }
    } else if let Some(j) = o.jobs.first()
        && let Some(e) = &j.error
    {
        lines.push(Line::from(vec![
            span("error ", Style::new().fg(t.dim)),
            span(fit(e, w.saturating_sub(6)), Style::new().fg(t.error)),
        ]));
    }

    let rel = relations(snap, o);
    let mut graph: Vec<String> = Vec::new();
    for r in rel.waits_on.iter().take(2) {
        graph.push(format!("waits on {} ({})", r.title, edge_label(r.kind)));
    }
    for r in rel.blocks.iter().take(2) {
        graph.push(format!("blocks {} ({})", r.title, edge_label(r.kind)));
    }
    if !graph.is_empty() {
        lines.push(Line::from(vec![
            span("graph ", Style::new().fg(t.dim)),
            span(
                fit(&graph.join(" · "), w.saturating_sub(6)),
                Style::new().fg(t.text),
            ),
        ]));
    }

    // The tabs.
    let mut tabs: Vec<Span<'static>> = Vec::new();
    for (i, tab) in Tab::ALL.into_iter().enumerate() {
        let label = format!(" {} {} ", i + 1, tab.title());
        tabs.push(if tab == app.work_screen.tab {
            span(
                label,
                Style::new()
                    .fg(t.on_accent)
                    .bg(t.lead)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            span(label, Style::new().fg(t.muted))
        });
        tabs.push(Span::raw(" "));
    }
    lines.push(Line::from(tabs));
    lines
}

// ---------------------------------------------------------------------
// Tabs
// ---------------------------------------------------------------------

fn body(app: &App, snap: &ConsoleSnapshot, o: &OutcomeView, w: usize) -> Lines {
    match app.work_screen.tab {
        Tab::Product => product::body(app, snap, o, w),
        Tab::Stream => stream(app, snap, o, w),
        Tab::Diff => diff(app, o, w),
        Tab::Checks => product::check_summary(app, o, w, true),
    }
}

/// The lead job's action stream, the rows Focus draws. A job from an
/// earlier run is not in memory: the Guard's last steps stand in, and Focus
/// replays the rest.
fn stream(app: &App, snap: &ConsoleSnapshot, o: &OutcomeView, w: usize) -> Lines {
    let t = &app.look.theme;
    let Some(job) = o.jobs.first() else {
        return vec![Line::from(span(
            "no job has run for this outcome yet",
            Style::new().fg(t.muted),
        ))];
    };
    let Some(progress) = app.engine.jobs.progress(JobId(job.id)) else {
        let mut out = vec![Line::from(span(
            format!(
                "job #{} ran before this session: ⏎ replays it in Focus",
                job.id
            ),
            Style::new().fg(t.muted),
        ))];
        for h in holds_of(snap, o) {
            for s in &h.steps {
                out.push(Line::from(span(
                    fit(&format!("{} {}", super::util::clock(s.at), s.text), w),
                    Style::new().fg(t.text),
                )));
            }
        }
        return out;
    };
    let all = rows(&progress.log);
    if all.is_empty() {
        return vec![Line::from(span(
            "waiting for the first action",
            Style::new().fg(t.muted),
        ))];
    }
    let mut out = vec![Line::from(span(
        format!("{} rows · job #{} · ⏎ opens it in Focus", all.len(), job.id),
        Style::new().fg(t.dim),
    ))];
    out.extend(
        all.iter()
            .filter(|r| !matches!(r.kind, RowKind::Note) || r.depth == 0)
            .map(|r| row_line_of(app, (&progress.model, &[]), r, w)),
    );
    out
}

/// Totals once the evidence is sealed, and the uncommitted changes of the
/// outcome's session, tinted as Focus tints them.
fn diff(app: &App, o: &OutcomeView, w: usize) -> Lines {
    let t = &app.look.theme;
    let mut out: Lines = Vec::new();
    match &o.diff {
        Some(d) => out.push(Line::from(vec![
            span("sealed diff ", Style::new().fg(t.muted)),
            span(format!("{} files ", d.files), Style::new().fg(t.text)),
            span(format!("+{} ", d.insertions), Style::new().fg(t.ok)),
            span(format!("-{}", d.deletions), Style::new().fg(t.error)),
            span(
                format!(" · digest {}", crate::work::short(&d.hash)),
                Style::new().fg(t.muted),
            ),
        ])),
        None => out.push(Line::from(span(
            "the sealed diff totals appear when the evidence is sealed",
            Style::new().fg(t.muted),
        ))),
    }
    out.push(Line::default());
    match &app.work_screen.diff {
        Some(read) if !read.lines.is_empty() => {
            out.push(heading(
                app,
                "UNCOMMITTED CHANGES",
                "of the job's session",
                w,
                t.label,
            ));
            let text = read.lines.join("\n");
            for (kind, line) in diff_lines(&text) {
                let line = fit(line, w);
                let pad = w.saturating_sub(line.chars().count());
                out.push(Line::from(span(
                    format!("{line}{}", " ".repeat(pad)),
                    diff_style(t, kind),
                )));
            }
        }
        Some(read) => out.push(Line::from(span(
            read.note
                .clone()
                .unwrap_or_else(|| "nothing to show".into()),
            Style::new().fg(t.muted),
        ))),
        None => out.push(Line::from(span(
            "press the diff key again to read the session's uncommitted changes",
            Style::new().fg(t.muted),
        ))),
    }
    out
}

// ---------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------

/// The ACTIONS panel: the key of each action as the keymap has it now,
/// what it does, or why it cannot be done.
fn actions(app: &App, rows: &[crate::work::ActionRow], w: usize) -> Lines {
    let t = &app.look.theme;
    if rows.is_empty() {
        return Vec::new();
    }
    let mut out = vec![heading(
        app,
        "ACTIONS",
        "",
        w,
        queue::group_color(t, GroupKind::Decide),
    )];
    let first_enabled = rows.iter().position(|r| r.enabled);
    for (i, r) in rows.iter().enumerate() {
        let key = app.key_of(r.action).unwrap_or_else(|| "·".to_string());
        let key_style = if r.enabled {
            if Some(i) == first_enabled {
                Style::new()
                    .fg(t.on_accent)
                    .bg(t.warn)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(t.title).add_modifier(Modifier::BOLD)
            }
        } else {
            Style::new().fg(t.dim)
        };
        let label_style = if r.enabled {
            Style::new().fg(t.title).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(t.dim)
        };
        let detail_w = w.saturating_sub(7 + 16);
        out.push(Line::from(vec![
            span(format!(" {key:<3}"), key_style),
            span(format!(" {:<15}", r.label), label_style),
            span(
                fit(&r.detail, detail_w),
                Style::new().fg(if r.enabled { t.muted } else { t.dim }),
            ),
        ]));
    }
    out
}
