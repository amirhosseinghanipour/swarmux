//! The Return brief drawn: while you were away, what landed with proof, what
//! is ready to land, the decisions waiting, what stopped, spend, the
//! Guard's exceptions, what did not run in full, what lands where, and a
//! short timeline.
//!
//! Drawn from the [`BriefView`] the app built; drawing does no I/O. A
//! figure there is no data for is not drawn as zero. At a small size the
//! sections fold, in this order of priority (kept first): decisions,
//! landed and ready, stopped, the Guard, spend and checks; then, behind
//! `Tab`: the timeline and what lands where.

#[cfg(test)]
mod tests;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_application::console::{
    BriefState, BriefView, GuardCallState, Lifecycle, MomentTone, PriorityKind,
};
use swarmux_domain::telemetry::local_clock;

use super::deck::{Want, frame as panel_frame, heights};
use super::work::util::{Lines, Mark, fit, lifecycle_word, mark, priority_color, span, usd};
use crate::app::App;
use crate::theme::Theme;
use crate::work::countdown;

/// Below this the sections fold.
const WIDE_WIDTH: u16 = 140;
const WIDE_HEIGHT: u16 = 40;
/// The smallest window the brief is drawn in.
const MIN_WIDTH: u16 = 80;
const MIN_HEIGHT: u16 = 16;
/// The bottom row of a wide window: timeline and what lands where.
const FOOT_HEIGHT: u16 = 12;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    match &app.brief.view {
        Some(view) => render_with(frame, area, app, view),
        None => frame.render_widget(
            Paragraph::new(Line::from(span(
                "reading the night…",
                Style::new().fg(t.muted),
            ))),
            area,
        ),
    }
}

/// Whether the sections fold at this size.
pub fn folds(width: u16, height: u16) -> bool {
    width < WIDE_WIDTH || height < WIDE_HEIGHT
}

pub(crate) fn render_with(frame: &mut Frame, area: Rect, app: &App, view: &BriefView) {
    let t = &app.look.theme;
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(span("make the window bigger", Style::new().fg(t.label))),
                Line::from(span(
                    format!("the brief needs {MIN_WIDTH}×{MIN_HEIGHT}"),
                    Style::new().fg(t.dim),
                )),
            ])
            .centered(),
            area,
        );
        return;
    }
    // The approval and typed-confirm overlays sit on the bottom rows of the
    // body: leave them room, so no section is hidden under them.
    let reserved = if matches!(app.mode, crate::app::Mode::Confirm(_)) {
        crate::ui::confirm::HEIGHT.min(area.height)
    } else if app.approval_target().is_some() {
        crate::ui::overlay_rows(app, area.height)
    } else {
        0
    };
    let area = Rect {
        height: area.height.saturating_sub(reserved),
        ..area
    };
    let folded = folds(area.width, area.height);
    let head_h = 5u16.min(area.height);
    let [head, body] =
        Layout::vertical([Constraint::Length(head_h), Constraint::Fill(1)]).areas(area);
    header(frame, head, app, view);

    match view.state {
        BriefState::FirstRun => first_run(frame, body, app, view),
        BriefState::Nothing => nothing(frame, body, app, view),
        BriefState::Happened => {
            if folded && app.brief.more {
                folded_sections(frame, body, app, view);
            } else if folded {
                main_sections(frame, body, app, view, true);
            } else {
                // What lands where, as tall as its rows; the timeline sits
                // under the decisions when they leave room, else beside it.
                let rows: usize = view.lands_where.iter().map(|r| r.items.len()).sum();
                let lands_h = u16::try_from(rows + 2)
                    .unwrap_or(FOOT_HEIGHT)
                    .clamp(4, FOOT_HEIGHT);
                let tall = body.height.saturating_sub(lands_h.min(body.height / 3));
                if timeline_under_decisions(view, tall, false) {
                    let foot = lands_h.min(body.height / 3);
                    let [main, foot_area] =
                        Layout::vertical([Constraint::Fill(1), Constraint::Length(foot)])
                            .areas(body);
                    main_sections(frame, main, app, view, false);
                    lands_where(frame, foot_area, app, view);
                } else {
                    let foot = FOOT_HEIGHT.min(body.height / 3);
                    let [main, foot_area] =
                        Layout::vertical([Constraint::Fill(1), Constraint::Length(foot)])
                            .areas(body);
                    main_sections(frame, main, app, view, false);
                    let [tl, lw] = Layout::horizontal([
                        Constraint::Percentage(50),
                        Constraint::Percentage(50),
                    ])
                    .areas(foot_area);
                    timeline(frame, tl, app, view);
                    lands_where(frame, lw, app, view);
                }
            }
        }
    }
}

/// Whether the timeline fits under the decisions in a column `height` tall
/// (nothing is running to take that room, and the size does not fold).
fn timeline_under_decisions(view: &BriefView, height: u16, folded: bool) -> bool {
    let need: usize = view
        .decisions
        .iter()
        .map(|d| 4 + usize::from(d.detail.is_some()))
        .sum::<usize>()
        + 2;
    let spare = usize::from(height).saturating_sub(need.max(6));
    view.running.is_empty() && !folded && !view.timeline.is_empty() && spare >= 6
}

// ---------------------------------------------------------------------
// Words
// ---------------------------------------------------------------------

/// `45s`, `12m`, `3h04m`, `2d4h`.
fn span_text(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
        _ => format!("{}d{}h", s / 86_400, s % 86_400 / 3600),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The first twelve characters of a bundle's hash.
fn short_hash(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

fn tone_color(t: &Theme, tone: MomentTone) -> ratatui::style::Color {
    match tone {
        MomentTone::Good => t.ok,
        MomentTone::Warn => t.warn,
        MomentTone::Bad => t.error,
        MomentTone::Plain => t.muted,
    }
}

fn muted(t: &Theme) -> Style {
    Style::new().fg(t.muted)
}

fn bold(color: ratatui::style::Color) -> Style {
    Style::new().fg(color).add_modifier(Modifier::BOLD)
}

fn paragraph(frame: &mut Frame, area: Rect, lines: Lines) {
    frame.render_widget(Paragraph::new(lines), area);
}

/// A framed panel and the room inside it, or `None` when there is none.
fn open(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    title: &str,
    note: &str,
    color: ratatui::style::Color,
) -> Option<Rect> {
    let block = panel_frame(app, title, note, color);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    (inner.width >= 12 && inner.height >= 1).then_some(inner)
}

// ---------------------------------------------------------------------
// The header
// ---------------------------------------------------------------------

fn header(frame: &mut Frame, area: Rect, app: &App, view: &BriefView) {
    let t = &app.look.theme;
    let w = usize::from(area.width);
    let offset = (app.utc_offset)();
    let mut lines: Lines = vec![Line::default()];

    // The window, from and to, and how long it was.
    let mut title = vec![span("WHILE YOU WERE AWAY", bold(t.title))];
    match &view.window {
        Some(win) => title.push(span(
            format!(
                "  {} → {} · {}",
                local_clock(win.from_at, offset),
                local_clock(win.to_at, offset),
                span_text(i64::try_from(win.duration_ms).unwrap_or(i64::MAX))
            ),
            muted(t),
        )),
        None => title.push(span("  no earlier run to compare with", muted(t))),
    }
    lines.push(Line::from(title));

    // The counts, each in the colour of what it means.
    let mut counts: Vec<Span<'static>> = Vec::new();
    let mut count = |n: usize, text: String, color: ratatui::style::Color| {
        if !counts.is_empty() {
            counts.push(span("   ", muted(t)));
        }
        counts.push(span(n.to_string(), bold(color)));
        counts.push(span(format!(" {text}"), Style::new().fg(t.text)));
    };
    if view.window.is_some() {
        let landed = (view.landed.with_proof + view.landed.without_proof) as usize;
        count(
            landed,
            if view.landed.without_proof == 0 {
                "landed with proof".to_string()
            } else {
                format!("landed ({} with proof)", view.landed.with_proof)
            },
            if landed > 0 { t.ok } else { t.dim },
        );
    }
    count(
        view.ready_total,
        "verified, ready to land".into(),
        if view.ready_total > 0 { t.ok } else { t.dim },
    );
    count(
        view.decisions_total,
        if view.decisions_total == 1 {
            "decision waiting".into()
        } else {
            "decisions waiting".into()
        },
        if view.decisions_total > 0 {
            t.warn
        } else {
            t.dim
        },
    );
    if view.window.is_some() {
        count(
            view.stopped_total,
            "stopped".into(),
            if view.stopped_total > 0 {
                t.error
            } else {
                t.dim
            },
        );
        match view.spend {
            Some(s) => {
                counts.push(span("   ", muted(t)));
                counts.push(span(usd(s.usd), bold(t.text)));
                counts.push(span(" spent", Style::new().fg(t.text)));
            }
            None => {
                counts.push(span("   spend unknown", Style::new().fg(t.dim)));
            }
        }
    }
    lines.push(Line::from(counts));

    // The verification figures, with how many they are over.
    if view.window.is_some() {
        lines.push(Line::from(span(fit(&figures(view), w), muted(t))));
    }
    paragraph(frame, area, lines);
}

/// What the ledger says about verification over the window, with the
/// size of the sample beside each figure.
fn figures(view: &BriefView) -> String {
    let mut parts: Vec<String> = Vec::new();
    match &view.verification {
        Some(v) => {
            match v.claim_to_verified {
                Some(s) => parts.push(format!(
                    "claim → verified: median {} (n {})",
                    span_text(i64::try_from(s.median_ms).unwrap_or(i64::MAX)),
                    s.samples
                )),
                None => parts.push("claim → verified: no claim was verified".into()),
            }
            match v.first_run {
                Some(f) => parts.push(format!(
                    "first-run pass {:.0}% ({} of {})",
                    f.rate * 100.0,
                    f.passed,
                    f.judged
                )),
                None => parts.push("first-run pass: no first claim was judged".into()),
            }
        }
        None if view.read.is_some() => {
            parts.push("verification: no claim was judged while you were away".into());
        }
        None => parts.push("verification figures: the ledger was not read".into()),
    }
    let landed = view.landed.with_proof + view.landed.without_proof;
    if landed > 0 {
        parts.push(format!(
            "{} of {} landings carry an evidence bundle",
            view.landed.with_proof, landed
        ));
    }
    if let Some(r) = &view.read
        && !r.complete
    {
        parts.push("only the newest part of the window was read".into());
    }
    parts.join(" · ")
}

// ---------------------------------------------------------------------
// States with little to show
// ---------------------------------------------------------------------

fn first_run(frame: &mut Frame, area: Rect, app: &App, view: &BriefView) {
    let t = &app.look.theme;
    let mut lines: Lines = vec![
        Line::from(span(
            "This is the first run here: there is no earlier session to compare with.",
            Style::new().fg(t.text),
        )),
        Line::from(span(
            format!(
                "After you have been away for {} minutes this screen says what happened meanwhile.",
                app.brief.settings.away_after_minutes
            ),
            muted(t),
        )),
    ];
    if view.decisions_total == 0 && view.ready_total == 0 {
        lines.push(Line::from(span(
            "Nothing waits for you now.",
            Style::new().fg(t.ok),
        )));
        paragraph(frame, area, lines);
        return;
    }
    let [intro, rest] = Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(area);
    paragraph(frame, intro, lines);
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(54), Constraint::Percentage(46)]).areas(rest);
    decisions(frame, left, app, view);
    if view.ready_total > 0 {
        let Some(inner) = open(
            frame,
            right,
            app,
            &format!("VERIFIED, READY TO LAND {}", view.ready_total),
            "",
            t.ok,
        ) else {
            return;
        };
        let mut rows: Lines = view
            .ready
            .iter()
            .map(|r| {
                Line::from(vec![
                    span(
                        format!("{} ", mark(app, Mark::Ready)),
                        Style::new().fg(t.ok),
                    ),
                    span(
                        fit(&r.title, usize::from(inner.width).saturating_sub(2)),
                        Style::new().fg(t.text),
                    ),
                ])
            })
            .collect();
        rows.truncate(usize::from(inner.height));
        paragraph(frame, inner, rows);
    }
}

fn nothing(frame: &mut Frame, area: Rect, app: &App, view: &BriefView) {
    let t = &app.look.theme;
    let mut lines: Lines = vec![
        Line::from(span(
            "Nothing happened while you were away, and nothing waits for you.",
            Style::new().fg(t.ok),
        )),
        Line::from(span(
            "No outcome landed or stopped, no call was held or refused, and no job ended with a cost.",
            muted(t),
        )),
    ];
    if let Some(r) = &view.read {
        lines.push(Line::from(span(
            format!(
                "{} read from the ledger{}.",
                plural(r.entries as usize, "entry", "entries"),
                if r.complete {
                    ""
                } else {
                    " (not the whole window)"
                }
            ),
            muted(t),
        )));
    }
    paragraph(frame, area, lines);
}

// ---------------------------------------------------------------------
// The main sections
// ---------------------------------------------------------------------

/// Decisions on the left; landed, stopped, the Guard and spend on the
/// right, each as many rows as it wants and the room allows. When the size
/// folds sections, what is left of the right column goes to the timeline
/// (if it has room) or to a line naming what folded.
fn main_sections(frame: &mut Frame, area: Rect, app: &App, view: &BriefView, folded: bool) {
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(54), Constraint::Percentage(46)]).areas(area);
    left_column(frame, left, app, view, folded);

    let w = usize::from(right.width).saturating_sub(2);
    let landed_rows = landed_lines(app, view, w).len();
    let stopped_rows = stopped_lines(app, view, w).len();
    let guard_rows = guard_lines(app, view, w).len();
    let spend_rows = spend_lines(app, view, w).len();
    let want = |rows: usize, grow: u16| {
        let ideal = u16::try_from(rows + 2).unwrap_or(u16::MAX);
        Want {
            min: 4.min(ideal),
            ideal,
            grow: if folded { 0 } else { grow },
        }
    };
    let mut wants = vec![want(landed_rows, 1)];
    if stopped_rows > 0 {
        wants.push(want(stopped_rows, 0));
    }
    wants.push(want(guard_rows, 1));
    wants.push(want(spend_rows, 0));
    // The last row is for what folded, when a size folds sections.
    let reserve = u16::from(folded);
    let rows = heights(right.height.saturating_sub(reserve), &wants);
    let mut y = right.y;
    let mut at = 0usize;
    let mut next = |at: &mut usize| {
        let h = rows[*at];
        *at += 1;
        let r = Rect {
            y,
            height: h,
            ..right
        };
        y += h;
        r
    };
    landed(frame, next(&mut at), app, view);
    if stopped_rows > 0 {
        stopped(frame, next(&mut at), app, view);
    }
    guard(frame, next(&mut at), app, view);
    spend(frame, next(&mut at), app, view);
    if !folded {
        return;
    }
    let spare = right.y + right.height - y;
    // A timeline of its own needs a border and a few lines.
    if spare >= 6 {
        let r = Rect {
            y,
            height: spare - 1,
            ..right
        };
        timeline(frame, r, app, view);
        y += spare - 1;
    }
    let r = Rect {
        y,
        height: 1,
        ..right
    };
    let key = app
        .key_of(crate::screens::brief::MORE)
        .unwrap_or_else(|| "tab".into());
    paragraph(
        frame,
        r,
        vec![Line::from(span(
            fit(
                &format!(" {key}: the timeline and what lands where, in full"),
                usize::from(r.width),
            ),
            Style::new().fg(app.look.theme.dim),
        ))],
    );
}

/// The decisions at the height they need, and what is still running in
/// the room below them (nothing to do there, but it says what the agents
/// are at).
fn left_column(frame: &mut Frame, area: Rect, app: &App, view: &BriefView, folded: bool) {
    let need: usize = view
        .decisions
        .iter()
        .map(|d| 4 + usize::from(d.detail.is_some()))
        .sum::<usize>()
        + 2;
    let spare = usize::from(area.height).saturating_sub(need.max(6));
    let timeline_below = timeline_under_decisions(view, area.height, folded);
    if (view.running.is_empty() && !timeline_below) || spare < 5 {
        decisions(frame, area, app, view);
        return;
    }
    let top = area.height - u16::try_from(spare).unwrap_or(0);
    let [a, b] = Layout::vertical([Constraint::Length(top), Constraint::Fill(1)]).areas(area);
    decisions(frame, a, app, view);
    if timeline_below {
        timeline(frame, b, app, view);
    } else {
        running(frame, b, app, view);
    }
}

fn running(frame: &mut Frame, area: Rect, app: &App, view: &BriefView) {
    let t = &app.look.theme;
    let Some(inner) = open(
        frame,
        area,
        app,
        &format!("STILL RUNNING {}", view.running_total),
        "no action needed",
        t.border,
    ) else {
        return;
    };
    let w = usize::from(inner.width);
    let now = app.now();
    let mut rows: Lines = view
        .running
        .iter()
        .map(|r| {
            Line::from(vec![
                span(
                    format!("{} ", mark(app, Mark::Open)),
                    Style::new().fg(t.label),
                ),
                span(fit(&r.title, w.saturating_sub(16)), Style::new().fg(t.text)),
                span(
                    format!(
                        "  {} · {}",
                        lifecycle_word(r.lifecycle),
                        span_text(now - r.since_at)
                    ),
                    muted(t),
                ),
            ])
        })
        .collect();
    rows.truncate(usize::from(inner.height));
    paragraph(frame, inner, rows);
}

/// The sections that fold at a small size, in the main ones' place.
fn folded_sections(frame: &mut Frame, area: Rect, app: &App, view: &BriefView) {
    let [top, bottom] =
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(area);
    timeline(frame, top, app, view);
    lands_where(frame, bottom, app, view);
}

fn decisions(frame: &mut Frame, area: Rect, app: &App, view: &BriefView) {
    let t = &app.look.theme;
    let n = view.decisions_total;
    let color = if n > 0 { t.warn } else { t.border };
    let Some(inner) = open(
        frame,
        area,
        app,
        &format!("DECISIONS WAITING {n}"),
        "ranked as in Work",
        color,
    ) else {
        return;
    };
    let w = usize::from(inner.width);
    let now = app.now();
    let mut lines: Lines = Vec::new();
    if view.decisions.is_empty() {
        lines.push(Line::from(span(
            "nothing waits for you",
            Style::new().fg(t.ok),
        )));
    }
    let room = usize::from(inner.height);
    let mut shown = 0usize;
    for (i, d) in view.decisions.iter().enumerate() {
        let rows = 3 + usize::from(d.detail.is_some()) + 1;
        // Keep a line for "+N more" when not everything fits.
        let last = i + 1 == view.decisions.len();
        let need = lines.len() + rows - 1 + if last { 0 } else { 1 };
        if need > room && shown > 0 {
            break;
        }
        shown += 1;
        let (m, color) = match d.priority {
            Some(PriorityKind::P1) => (mark(app, Mark::Bad), priority_color(t, PriorityKind::P1)),
            Some(p) => (mark(app, Mark::Ask), priority_color(t, p)),
            None => (mark(app, Mark::Ask), t.warn),
        };
        let tag = if i == 0 { "  start here " } else { "" };
        let title_w = w.saturating_sub(3 + tag.chars().count());
        let mut first = vec![
            span(format!("{m} "), Style::new().fg(color)),
            span(fit(&d.title, title_w), bold(t.text)),
        ];
        if i == 0 {
            first.push(span(
                tag,
                Style::new()
                    .fg(t.on_accent)
                    .bg(t.warn)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        lines.push(Line::from(first));
        lines.push(Line::from(span(
            format!("  {}", fit(&d.ask, w.saturating_sub(2))),
            Style::new().fg(t.label),
        )));
        if let Some(detail) = &d.detail {
            lines.push(Line::from(span(
                format!("  {}", fit(detail, w.saturating_sub(2))),
                muted(t),
            )));
        }
        // What resolves it by itself, or what waits on it.
        let third = if let Some(deadline) = d.deadline_at {
            span(
                format!("  auto-deny in {}", countdown(deadline, now)),
                Style::new().fg(t.warn),
            )
        } else if !d.blocks.is_empty() {
            span(
                fit(&format!("  blocks: {}", d.blocks.join(", ")), w),
                Style::new().fg(t.warn),
            )
        } else {
            span(
                format!("  waiting {}", span_text(now - d.since_at)),
                muted(t),
            )
        };
        lines.push(Line::from(third));
        lines.push(Line::default());
    }
    let hidden = n.saturating_sub(shown);
    if hidden > 0 {
        while lines.len() >= room && !lines.is_empty() {
            lines.pop();
        }
        lines.push(Line::from(span(
            format!("+{hidden} more, in Work"),
            muted(t),
        )));
    }
    paragraph(frame, inner, lines);
}

fn landed_lines(app: &App, view: &BriefView, w: usize) -> Lines {
    let t = &app.look.theme;
    let mut lines: Lines = Vec::new();
    if view.landed.items.is_empty() {
        lines.push(Line::from(span(
            "nothing landed while you were away",
            Style::new().fg(t.dim),
        )));
    }
    for i in &view.landed.items {
        let right = match (&i.bundle, i.files) {
            (Some(b), Some(f)) => format!(
                "{} · {} files +{} -{}",
                short_hash(b),
                f,
                i.insertions.unwrap_or(0),
                i.deletions.unwrap_or(0)
            ),
            (Some(b), None) => short_hash(b).to_string(),
            (None, _) => "no bundle".to_string(),
        };
        let title_w = w.saturating_sub(4 + right.chars().count());
        lines.push(Line::from(vec![
            span(format!("{} ", mark(app, Mark::Pass)), Style::new().fg(t.ok)),
            span(
                format!("{:<title_w$}  ", fit(&i.title, title_w)),
                Style::new().fg(t.text),
            ),
            span(right, muted(t)),
        ]));
    }
    if view.landed.truncated {
        lines.push(Line::from(span(
            "the queue lists only its newest landed outcomes",
            Style::new().fg(t.dim),
        )));
    }
    if !view.ready.is_empty() {
        lines.push(Line::from(span(
            format!("verified, ready to land · {}", view.ready_total),
            bold(t.ok),
        )));
        for r in &view.ready {
            lines.push(Line::from(vec![
                span(
                    format!("{} ", mark(app, Mark::Ready)),
                    Style::new().fg(t.ok),
                ),
                span(fit(&r.title, w.saturating_sub(2)), Style::new().fg(t.text)),
            ]));
        }
    }
    lines
}

fn landed(frame: &mut Frame, area: Rect, app: &App, view: &BriefView) {
    let t = &app.look.theme;
    let n = view.landed.with_proof + view.landed.without_proof;
    let note = if view.landed.without_proof == 0 && n > 0 {
        "evidence bundle each".to_string()
    } else {
        String::new()
    };
    let Some(inner) = open(
        frame,
        area,
        app,
        &format!("LANDED {n}"),
        &note,
        if n > 0 { t.ok } else { t.border },
    ) else {
        return;
    };
    let mut lines = landed_lines(app, view, usize::from(inner.width));
    lines.truncate(usize::from(inner.height));
    paragraph(frame, inner, lines);
}

fn stopped_lines(app: &App, view: &BriefView, w: usize) -> Lines {
    let t = &app.look.theme;
    view.stopped
        .iter()
        .map(|s| {
            let word = match s.lifecycle {
                Lifecycle::Escalated => "escalated",
                other => lifecycle_word(other),
            };
            let why = s.reason.as_deref().unwrap_or("no reason was kept");
            let tail = if s.decision_waiting {
                " · decision waiting"
            } else {
                ""
            };
            Line::from(vec![
                span(
                    format!("{} ", mark(app, Mark::Bad)),
                    Style::new().fg(t.error),
                ),
                span(
                    fit(
                        &format!("{} · {word} · {why}{tail}", s.title),
                        w.saturating_sub(2),
                    ),
                    Style::new().fg(t.text),
                ),
            ])
        })
        .collect()
}

fn stopped(frame: &mut Frame, area: Rect, app: &App, view: &BriefView) {
    let t = &app.look.theme;
    let Some(inner) = open(
        frame,
        area,
        app,
        &format!("STOPPED {}", view.stopped_total),
        "",
        t.error,
    ) else {
        return;
    };
    let mut lines = stopped_lines(app, view, usize::from(inner.width));
    lines.truncate(usize::from(inner.height));
    paragraph(frame, inner, lines);
}

/// Calls the Guard held or refused, with the runs of the same kind (same
/// state and reason) cut to the first few, and a count of the rest.
fn collapse(
    calls: &[swarmux_application::console::GuardCallView],
) -> (
    Vec<&swarmux_application::console::GuardCallView>,
    Vec<(&'static str, String, usize)>,
) {
    const KEEP: usize = 3;
    let word = |s: GuardCallState| match s {
        GuardCallState::Waiting => "held, waiting",
        GuardCallState::Held => "held",
        GuardCallState::HeldAllowed => "held, allowed",
        GuardCallState::HeldDenied => "held, denied",
        GuardCallState::Denied => "denied",
        GuardCallState::Stopped => "stopped",
    };
    let mut seen: Vec<(GuardCallState, &str, usize)> = Vec::new();
    let mut shown = Vec::new();
    for c in calls {
        match seen
            .iter_mut()
            .find(|(s, r, _)| *s == c.state && *r == c.reason)
        {
            Some((_, _, n)) => *n += 1,
            None => seen.push((c.state, c.reason.as_str(), 1)),
        }
        let n = seen
            .iter()
            .find(|(s, r, _)| *s == c.state && *r == c.reason)
            .map_or(0, |x| x.2);
        if n <= KEEP {
            shown.push(c);
        }
    }
    let more = seen
        .into_iter()
        .filter(|(_, _, n)| *n > KEEP)
        .map(|(s, r, n)| (word(s), r.to_string(), n - KEEP))
        .collect();
    (shown, more)
}

/// The Guard's exceptions only: held and refused calls with their rule or
/// reason, then the calls that fell back and the rules added; the routine
/// allows are one number.
fn guard_lines(app: &App, view: &BriefView, w: usize) -> Lines {
    let t = &app.look.theme;
    let offset = (app.utc_offset)();
    let mut lines: Lines = Vec::new();
    match &view.guard {
        None => lines.push(Line::from(span(
            "the ledger was not read: no figure",
            Style::new().fg(t.dim),
        ))),
        Some(g) => {
            lines.push(Line::from(span(
                format!(
                    "{} routine allows, not listed · {}",
                    g.allowed,
                    plural(g.calls_total, "exception", "exceptions")
                ),
                muted(t),
            )));
            let (shown, more) = collapse(&g.calls);
            for c in shown {
                let (m, word, color) = match c.state {
                    GuardCallState::Waiting => (mark(app, Mark::Ask), "held, waiting", t.warn),
                    GuardCallState::Held => (mark(app, Mark::Ask), "held", t.warn),
                    GuardCallState::HeldAllowed => (mark(app, Mark::Pass), "held, allowed", t.ok),
                    GuardCallState::HeldDenied => (mark(app, Mark::Bad), "held, denied", t.error),
                    GuardCallState::Denied => (mark(app, Mark::Bad), "denied", t.error),
                    GuardCallState::Stopped => (mark(app, Mark::Bad), "stopped", t.error),
                };
                let job = c.job.map_or(String::new(), |j| format!("#{j} "));
                let text = format!(
                    "{} {job}{} {} · {word} · {}",
                    local_clock(c.at, offset),
                    c.tool,
                    c.target,
                    c.reason
                );
                lines.push(Line::from(vec![
                    span(format!("{m} "), Style::new().fg(color)),
                    span(fit(&text, w.saturating_sub(2)), Style::new().fg(t.text)),
                ]));
            }
            for (word, reason, n) in more {
                lines.push(Line::from(vec![
                    span(format!("{} ", mark(app, Mark::Idle)), muted(t)),
                    span(
                        fit(
                            &format!("+{n} more, each {word} · {reason}: in Work or the Deck"),
                            w.saturating_sub(2),
                        ),
                        muted(t),
                    ),
                ]));
            }
        }
    }
    for f in &view.fallbacks {
        let job = f.job.map_or(String::new(), |j| format!("#{j} "));
        lines.push(Line::from(vec![
            span(
                format!("{} ", mark(app, Mark::Flag)),
                Style::new().fg(t.warn),
            ),
            span(
                fit(
                    &format!(
                        "{} {job}{} fell back ({}): {}",
                        local_clock(f.at, offset),
                        f.tool,
                        f.mode,
                        f.reason
                    ),
                    w.saturating_sub(2),
                ),
                Style::new().fg(t.text),
            ),
        ]));
    }
    for r in &view.rules_added {
        let scope = match &r.root {
            Some(root) => format!("{} {root}", r.scope),
            None => r.scope.clone(),
        };
        lines.push(Line::from(vec![
            span(
                format!("{} ", mark(app, Mark::Flag)),
                Style::new().fg(t.warn),
            ),
            span(
                fit(
                    &format!("always-allow added: {} · {scope}", r.rule),
                    w.saturating_sub(2),
                ),
                Style::new().fg(t.text),
            ),
        ]));
    }
    lines
}

fn guard(frame: &mut Frame, area: Rect, app: &App, view: &BriefView) {
    let t = &app.look.theme;
    let attention = view.guard.as_ref().is_some_and(|g| g.calls_total > 0)
        || !view.fallbacks.is_empty()
        || !view.rules_added.is_empty();
    let Some(inner) = open(
        frame,
        area,
        app,
        "GUARD WHILE AWAY",
        "exceptions only",
        if attention { t.warn } else { t.border },
    ) else {
        return;
    };
    let mut lines = guard_lines(app, view, usize::from(inner.width));
    lines.truncate(usize::from(inner.height));
    paragraph(frame, inner, lines);
}

/// What it cost, and what ran in less than full: hidden checks that did
/// not run and jobs checked in their own worktree.
fn spend_lines(app: &App, view: &BriefView, w: usize) -> Lines {
    let t = &app.look.theme;
    let mut lines: Lines = Vec::new();
    match view.spend {
        Some(s) => lines.push(Line::from(vec![
            span(usd(s.usd), bold(t.text)),
            span(
                format!(" while away · {}", plural(s.jobs as usize, "job", "jobs")),
                muted(t),
            ),
        ])),
        None => lines.push(Line::from(span(
            "spend while away: unknown (no job ended with a cost)",
            Style::new().fg(t.dim),
        ))),
    }
    let today = view.spend.and_then(|s| s.today_usd).map_or_else(
        || "today: unknown".to_string(),
        |d| format!("today {} across all sessions", usd(d)),
    );
    lines.push(Line::from(span(
        format!("{today} · day budget: not tracked"),
        muted(t),
    )));
    if let Some(c) = &view.checks {
        for h in &c.hidden_not_run {
            let who = h
                .title
                .clone()
                .or_else(|| h.job.map(|j| format!("#{j}")))
                .unwrap_or_default();
            lines.push(Line::from(vec![
                span(
                    format!("{} ", mark(app, Mark::Flag)),
                    Style::new().fg(t.warn),
                ),
                span(
                    fit(
                        &format!("hidden checks not run ({}): {who}", h.reason),
                        w.saturating_sub(2),
                    ),
                    Style::new().fg(t.text),
                ),
            ]));
        }
        for r in &c.worktree_runs {
            let who = r
                .title
                .clone()
                .or_else(|| r.job.map(|j| format!("#{j}")))
                .unwrap_or_default();
            lines.push(Line::from(vec![
                span(
                    format!("{} ", mark(app, Mark::Flag)),
                    Style::new().fg(t.warn),
                ),
                span(
                    fit(
                        &format!("checked in the worktree, not a clean copy: {who}"),
                        w.saturating_sub(2),
                    ),
                    Style::new().fg(t.text),
                ),
            ]));
        }
    }
    lines.extend(back_pressure_lines(app, view));
    lines
}

/// Where the lines of review back pressure go once it exists (milestone
/// X5): "dispatch held by WIP limit since …" and "sampled items waiting".
/// Nothing is drawn until then, and nothing stands in for it.
fn back_pressure_lines(app: &App, view: &BriefView) -> Lines {
    let t = &app.look.theme;
    let Some(bp) = &view.back_pressure else {
        return Vec::new();
    };
    let offset = (app.utc_offset)();
    let mut lines: Lines = Vec::new();
    if let Some(since) = bp.held_since {
        lines.push(Line::from(span(
            format!(
                "dispatch held by WIP limit since {}",
                local_clock(since, offset)
            ),
            Style::new().fg(t.warn),
        )));
    }
    if let Some(n) = bp.sampled_waiting {
        lines.push(Line::from(span(
            format!("{} sampled items waiting", n),
            Style::new().fg(t.warn),
        )));
    }
    lines
}

fn spend(frame: &mut Frame, area: Rect, app: &App, view: &BriefView) {
    let t = &app.look.theme;
    let Some(inner) = open(frame, area, app, "SPEND AND CHECKS", "", t.border) else {
        return;
    };
    let mut lines = spend_lines(app, view, usize::from(inner.width));
    lines.truncate(usize::from(inner.height));
    paragraph(frame, inner, lines);
}

// ---------------------------------------------------------------------
// The foot
// ---------------------------------------------------------------------

fn timeline(frame: &mut Frame, area: Rect, app: &App, view: &BriefView) {
    let t = &app.look.theme;
    let Some(inner) = open(frame, area, app, "TIMELINE", "oldest first", t.border) else {
        return;
    };
    let w = usize::from(inner.width);
    let offset = (app.utc_offset)();
    if view.timeline.is_empty() {
        paragraph(
            frame,
            inner,
            vec![Line::from(span(
                "nothing is recorded in the window",
                Style::new().fg(t.dim),
            ))],
        );
        return;
    }
    // The newest that fit, shown oldest first.
    let room = usize::from(inner.height);
    let skip = view.timeline.len().saturating_sub(room);
    let lines: Lines = view
        .timeline
        .iter()
        .skip(skip)
        .map(|m| {
            let color = tone_color(t, m.tone);
            let who = m.job.map_or_else(String::new, |j| format!("#{j}"));
            Line::from(vec![
                span(format!("{} ", local_clock(m.at, offset)), muted(t)),
                span(format!("{:<5} ", who), Style::new().fg(t.label)),
                span(fit(&m.text, w.saturating_sub(12)), Style::new().fg(color)),
            ])
        })
        .collect();
    paragraph(frame, inner, lines);
}

fn lands_where(frame: &mut Frame, area: Rect, app: &App, view: &BriefView) {
    let t = &app.look.theme;
    let Some(inner) = open(
        frame,
        area,
        app,
        "WHAT LANDS WHERE",
        "by repository, from landing data",
        t.border,
    ) else {
        return;
    };
    let w = usize::from(inner.width);
    if view.lands_where.is_empty() {
        paragraph(
            frame,
            inner,
            vec![Line::from(span(
                "no outcome landed to a repository while you were away",
                Style::new().fg(t.dim),
            ))],
        );
        return;
    }
    let repo_w = view
        .lands_where
        .iter()
        .map(|r| r.repo.chars().count())
        .max()
        .unwrap_or(0)
        .min(w / 3);
    let mut lines: Lines = Vec::new();
    for r in &view.lands_where {
        for (i, item) in r.items.iter().enumerate() {
            let repo = if i == 0 {
                fit(&r.repo, repo_w)
            } else {
                String::new()
            };
            let commit = item
                .commit
                .as_deref()
                .map(|c| format!(" · {}", c.get(..8).unwrap_or(c)))
                .unwrap_or_default();
            lines.push(Line::from(vec![
                span(format!("{repo:<repo_w$}  "), Style::new().fg(t.label)),
                span(
                    fit(
                        &format!("{}{commit}", item.title),
                        w.saturating_sub(repo_w + 2),
                    ),
                    Style::new().fg(t.text),
                ),
            ]));
        }
    }
    lines.truncate(usize::from(inner.height));
    paragraph(frame, inner, lines);
}
