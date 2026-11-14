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
use crate::app::{App, InputPurpose, Mode};
use crate::focus::stream::diff_lines;
use crate::focus::stream::{RowKind, rows};
use crate::ui::focus::parts::diff_style;
use crate::ui::focus::stream::row_line_of;
use crate::ui::mission::age;
use crate::work::{
    MessageComposer, NoteComposer, NoteTarget, Tab, edge_label, holds_of, relations,
};
use swarmux_domain::intervene::{Mode as Via, Receipt, State};

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
    // While a note or a message is being written its panel takes the
    // actions' place.
    let actions = match (&app.work_screen.message, &app.work_screen.note) {
        (Some(composer), _) => message_panel(app, composer, w),
        (None, Some(composer)) => note_panel(app, composer, w),
        (None, None) => actions(app, &crate::work::rows_for(snap, o), w),
    };
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
        let waiting = app.engine.interventions.waiting(JobId(j.id));
        if waiting > 0 {
            facts.push(format!(
                "{waiting} message{} queued",
                if waiting == 1 { "" } else { "s" }
            ));
        }
    }
    facts.push(usd(o.spend.total_usd));
    facts.push(format!("{} in state", age(now - o.since_at)));
    if let Some(due) = o.contract.as_ref().and_then(|c| c.due) {
        facts.push(super::util::due_text(due, now, (app.utc_offset)()));
    }
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
    let mut out: Lines = Vec::new();
    if all.is_empty() {
        out.push(Line::from(span(
            "waiting for the first action",
            Style::new().fg(t.muted),
        )));
    } else {
        out.push(Line::from(span(
            format!("{} rows · job #{} · ⏎ opens it in Focus", all.len(), job.id),
            Style::new().fg(t.dim),
        )));
        out.extend(
            all.iter()
                .filter(|r| !matches!(r.kind, RowKind::Note) || r.depth == 0)
                .map(|r| row_line_of(app, (&progress.model, &[]), r, w)),
        );
    }
    out.extend(messages(app, JobId(job.id), w));
    out
}

/// What was said to the agent and where each message stands: the other
/// half of the conversation. A receipt says "delivered" only once the
/// agent's turn with the message started; until then it says what is not
/// known.
fn messages(app: &App, job: JobId, w: usize) -> Lines {
    let t = &app.look.theme;
    let receipts: Vec<&Receipt> = app.engine.interventions.involving(job).collect();
    if receipts.is_empty() {
        return Vec::new();
    }
    let mut out: Lines = vec![
        Line::default(),
        heading(
            app,
            "YOUR MESSAGES",
            "to the agent, in your words",
            w,
            t.lead,
        ),
    ];
    for r in receipts {
        let mode = match r.mode {
            Via::Queue => "queue",
            Via::Steer => "steer",
        };
        out.push(Line::from(vec![
            span(format!(" #{} ", r.id), Style::new().fg(t.dim)),
            span(
                format!("{mode:<6}"),
                Style::new().fg(t.lead).add_modifier(Modifier::BOLD),
            ),
            span(
                fit(&r.text.replace('\n', " "), w.saturating_sub(12)),
                Style::new().fg(t.title),
            ),
        ]));
        let (hue, line) = match &r.state {
            State::Queued => (
                t.warn,
                "queued · delivers when the agent's turn ends".to_string(),
            ),
            State::Handed { job } => (
                t.muted,
                format!("handed to the agent as job #{job} · not confirmed until its turn starts"),
            ),
            State::Delivered { job, at } => (
                t.ok,
                format!(
                    "delivered · its turn started as job #{job} at {}",
                    super::util::clock(*at)
                ),
            ),
            State::Undelivered(why) => (t.error, format!("not delivered · {why}")),
            State::Refused(why) => (t.error, format!("not sent · {}", why.reason())),
        };
        for (i, part) in super::util::wrap_text(&line, w.saturating_sub(5))
            .into_iter()
            .enumerate()
        {
            out.push(Line::from(span(
                format!("{}{part}", if i == 0 { "      " } else { "        " }),
                Style::new().fg(hue),
            )));
        }
    }
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
    // The files of the change with the lines git counted in each: from the
    // sealed bundle, else from the check against the plan. A bundle sealed
    // before they were kept has only the totals above, and says so.
    if !o.files.is_empty() {
        let source = if o.diff.is_some() {
            "from the sealed bundle"
        } else {
            "counted when the build was checked against its plan"
        };
        out.push(heading(app, "FILES", source, w, t.label));
        out.extend(product::file_rows(app, &o.files, w, 2, 40));
        out.push(Line::default());
    } else if o.diff.is_some() {
        out.push(Line::from(span(
            "no per-file counts in this bundle: it was sealed before they were kept, the totals are all it has",
            Style::new().fg(t.muted),
        )));
        out.push(Line::default());
    }
    match &app.work_screen.diff {
        Some(read) if !read.lines.is_empty() => {
            out.push(heading(
                app,
                "UNCOMMITTED CHANGES",
                "of the job's session",
                w,
                t.label,
            ));
            out.extend(product::file_rows(app, &read.files, w, 2, 40));
            if !read.files.is_empty() {
                out.push(Line::default());
            }
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

// ---------------------------------------------------------------------
// The note panel
// ---------------------------------------------------------------------

/// What the user is writing to the agent, and the suggested notes they can
/// start from. The line itself is the status bar's, as for every other
/// text input; this says where the note goes and what happens after.
fn note_panel(app: &App, composer: &NoteComposer, w: usize) -> Lines {
    let t = &app.look.theme;
    let held = matches!(composer.target, NoteTarget::Held(_));
    let (title, goes) = if held {
        (
            "DENY WITH A NOTE",
            format!(
                "{} is denied; your note is the reason the agent reads",
                composer.about
            ),
        )
    } else {
        (
            "SEND BACK",
            format!(
                "{} resumes in its own session with your note as its next instruction; the checks run again on its next claim",
                composer.about
            ),
        )
    };
    let mut out = vec![heading(app, title, "", w, t.warn)];
    for line in super::util::wrap_text(&goes, w.saturating_sub(2)) {
        out.push(Line::from(span(
            format!(" {line}"),
            Style::new().fg(t.text),
        )));
    }
    // What has been typed so far, whole: the line at the bottom of the
    // screen shares its row with the status, and a long note would run
    // under it.
    let typed = match &app.mode {
        Mode::Input(InputPurpose::SendBackNote, input) => input.text.as_str(),
        _ => "",
    };
    let mut note = super::util::wrap_text(typed, w.saturating_sub(9));
    if typed.is_empty() {
        note = vec!["type below".to_string()];
    }
    for (i, line) in note.iter().take(3).enumerate() {
        out.push(Line::from(vec![
            span(
                if i == 0 { " note   " } else { "        " },
                Style::new().fg(t.muted),
            ),
            span(
                line.clone(),
                Style::new().fg(if typed.is_empty() { t.dim } else { t.title }),
            ),
        ]));
    }
    out.push(Line::from(span(
        if composer.suggestions.is_empty() {
            " enter sends it · esc drops it · nothing is sent before you press enter"
        } else {
            " enter sends it · esc drops it · tab picks a suggestion · nothing is sent before you press enter"
        },
        Style::new().fg(t.muted),
    )));
    for (i, s) in composer.suggestions.iter().enumerate() {
        // Marked while the line still holds it.
        let picked = composer.picked == Some(i) && s == typed;
        let key = if picked {
            Style::new()
                .fg(t.on_accent)
                .bg(t.warn)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(t.dim)
        };
        out.push(Line::from(vec![
            span(format!(" {} ", i + 1), key),
            span(
                format!(" {}", fit(s, w.saturating_sub(5))),
                Style::new().fg(if picked { t.title } else { t.text }),
            ),
        ]));
    }
    out
}

// ---------------------------------------------------------------------
// The message panel
// ---------------------------------------------------------------------

/// What the user is writing to the running agent, the mode it goes in and
/// what that mode does and does not do. The line itself is the status
/// bar's, as for every other text input.
fn message_panel(app: &App, composer: &MessageComposer, w: usize) -> Lines {
    let t = &app.look.theme;
    let steer = composer.mode == Via::Steer;
    let hue = if steer { t.warn } else { t.lead };
    let mut out = vec![heading(app, "MESSAGE THE AGENT", &composer.about, w, hue)];
    out.push(Line::from(vec![
        span(" mode   ", Style::new().fg(t.muted)),
        span(
            composer.mode.as_str().to_string(),
            Style::new().fg(hue).add_modifier(Modifier::BOLD),
        ),
        span("  (tab switches)", Style::new().fg(t.dim)),
    ]));
    for line in super::util::wrap_text(composer.mode.promise(), w.saturating_sub(2)) {
        out.push(Line::from(span(
            format!(" {line}"),
            Style::new().fg(t.text),
        )));
    }
    let held = app
        .engine
        .guard
        .approvals()
        .iter()
        .any(|a| a.request.job_id == Some(JobId(composer.job)));
    if held {
        let line = if steer {
            "a call is held for you: steering is refused until you answer it; queue works"
        } else {
            "a call is held for you: this message does not answer it"
        };
        for line in super::util::wrap_text(line, w.saturating_sub(2)) {
            out.push(Line::from(span(
                format!(" {line}"),
                Style::new().fg(t.warn),
            )));
        }
    }
    let typed = match &app.mode {
        Mode::Input(InputPurpose::WorkMessage, input) => input.text.as_str(),
        _ => "",
    };
    let mut text = super::util::wrap_text(typed, w.saturating_sub(10));
    if typed.is_empty() {
        text = vec!["type below".to_string()];
    }
    for (i, line) in text.iter().take(3).enumerate() {
        out.push(Line::from(vec![
            span(
                if i == 0 { " message " } else { "         " },
                Style::new().fg(t.muted),
            ),
            span(
                line.clone(),
                Style::new().fg(if typed.is_empty() { t.dim } else { t.title }),
            ),
        ]));
    }
    if composer.armed {
        out.push(Line::from(span(
            " enter again stops the agent's turn and sends it · esc drops it",
            Style::new().fg(t.warn).add_modifier(Modifier::BOLD),
        )));
    } else {
        out.push(Line::from(span(
            " enter sends it · esc drops it · tab switches the mode",
            Style::new().fg(t.muted),
        )));
        out.push(Line::from(span(
            " nothing is sent before you press enter",
            Style::new().fg(t.muted),
        )));
    }
    out
}
