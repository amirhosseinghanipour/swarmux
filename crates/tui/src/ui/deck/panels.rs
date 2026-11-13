//! The panels around the graph: what needs the user, how verification is
//! going, the limits and spend, what the Guard held or refused, and what
//! happened lately. Each shows what the snapshot (or the app's own feeds)
//! has, and says plainly when it has nothing; a figure there is no data
//! for is shown as unknown, never as zero.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_application::console::{
    ConsoleSnapshot, GroupKind, HoldState, Lifecycle, LimitWindow, MetricsView, OutcomeKind,
    OutcomeView, PriorityKind,
};
use swarmux_application::{ChangeKind, Outcome};
use swarmux_domain::console::Panel;
use swarmux_domain::keys::actions;
use swarmux_domain::telemetry::local_clock;

use super::frame;
use crate::app::App;
use crate::theme::Theme;
use crate::ui::look::IconSet;
use crate::ui::mission::age;
use crate::ui::widgets::bar_spans;
use crate::ui::work::util::{Lines, fit, priority_color, priority_text, rule, span, usd};
use crate::work::{countdown, edge_label, relations};

pub(super) fn render(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    snap: &ConsoleSnapshot,
    panel: Panel,
) {
    match panel {
        Panel::Needs => needs(frame, area, app, snap),
        Panel::Verification => verification(frame, area, app, snap),
        Panel::Limits => limits(frame, area, app, snap),
        Panel::Guard => guard(frame, area, app, snap),
        Panel::Timeline => timeline(frame, area, app),
        Panel::Graph | Panel::Sessions => {}
    }
}

/// The panel's frame drawn, and the room inside it; `None` when there is
/// none.
fn open(
    frame_: &mut Frame,
    area: Rect,
    app: &App,
    title: &str,
    note: &str,
    color: ratatui::style::Color,
) -> Option<Rect> {
    let block = frame(app, title, note, color);
    let inner = block.inner(area);
    frame_.render_widget(block, area);
    (inner.width >= 12 && inner.height >= 1).then_some(inner)
}

fn label_style(t: &Theme) -> Style {
    Style::new().fg(t.muted)
}

/// `key  value`, the key padded to `w`.
fn row(t: &Theme, key: &str, w: usize, value: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![span(format!("{key:<w$} "), label_style(t))];
    spans.extend(value);
    Line::from(spans)
}

fn unknown(t: &Theme, why: &str) -> Vec<Span<'static>> {
    vec![
        span("—", Style::new().fg(t.dim)),
        span(format!("  {why}"), Style::new().fg(t.dim)),
    ]
}

/// `45s`, `12m`, `3h05m`, `2d4h`: a span as short as it reads.
fn span_text(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
        _ => format!("{}d{}h", s / 86_400, s % 86_400 / 3600),
    }
}

// ---------------------------------------------------------------------
// NEEDS YOU
// ---------------------------------------------------------------------

fn decisions(snap: &ConsoleSnapshot) -> Vec<&OutcomeView> {
    snap.groups
        .iter()
        .filter(|g| g.kind == GroupKind::Decide)
        .flat_map(|g| &g.outcomes)
        .collect()
}

fn needs(frame_: &mut Frame, area: Rect, app: &App, snap: &ConsoleSnapshot) {
    let t = &app.look.theme;
    let all = decisions(snap);
    let n = snap.header.counts.decide;
    let color = if n > 0 { t.warn } else { t.border };
    let Some(inner) = open(
        frame_,
        area,
        app,
        "NEEDS YOU",
        &format!("{n} · in Work"),
        color,
    ) else {
        return;
    };
    let w = usize::from(inner.width);
    let mut lines: Lines = Vec::new();
    if all.is_empty() {
        lines.push(Line::from(span(
            "nothing waits for you",
            Style::new().fg(t.ok),
        )));
    }
    // Two rows an item; the last row is the way to Work.
    let room = usize::from(inner.height.saturating_sub(1)) / 2;
    let now = app.now();
    for o in all.iter().take(room.max(1)) {
        let bad = matches!(
            o.lifecycle,
            Lifecycle::Escalated | Lifecycle::Failed | Lifecycle::Refused
        );
        let (mark, mark_color) = match (bad, app.look.icons == IconSet::Ascii) {
            (true, false) => ("×", t.error),
            (true, true) => ("x", t.error),
            (false, false) => ("◆", t.warn),
            (false, true) => ("*", t.warn),
        };
        let p = o.priority.unwrap_or(PriorityKind::P3);
        let ptext = priority_text(p);
        lines.push(Line::from(vec![
            span(format!("{mark} "), Style::new().fg(mark_color)),
            span(
                format!(
                    "{:<pw$}",
                    fit(&o.title, w.saturating_sub(5)),
                    pw = w.saturating_sub(5)
                ),
                Style::new().fg(t.title).add_modifier(Modifier::BOLD),
            ),
            span(format!(" {ptext}"), Style::new().fg(priority_color(t, p))),
        ]));
        let mut why: Vec<String> = Vec::new();
        if let Some(d) = o.asks.iter().filter_map(|a| a.deadline_at).min() {
            why.push(format!("auto-deny in {}", countdown(d, now)));
        }
        let rel = relations(snap, o);
        if let Some(b) = rel.blocks.first() {
            why.push(format!("blocks {} ({})", b.title, edge_label(b.kind)));
        } else if let Some(a) = o.asks.first() {
            why.push(a.detail.lines().next().unwrap_or(&a.detail).to_string());
        }
        lines.push(Line::from(span(
            format!("  {}", fit(&why.join(" · "), w.saturating_sub(2))),
            Style::new().fg(t.warn),
        )));
    }
    let shown = usize::from(inner.height.saturating_sub(1));
    lines.truncate(shown.max(1));
    frame_.render_widget(Paragraph::new(lines), inner);
    let go = app
        .key_of(actions::GO_WORK)
        .unwrap_or_else(|| "`nav.go_work`".into());
    let enter = app
        .key_of(actions::LIST_OPEN)
        .map(|k| crate::hints::symbol(&k).to_string())
        .unwrap_or_else(|| "enter".into());
    let more = n.saturating_sub(room.max(1));
    let tail = if more > 0 && n > all.len().min(room.max(1)) {
        format!("+{more} more · ")
    } else {
        String::new()
    };
    frame_.render_widget(
        Paragraph::new(Line::from(span(
            fit(
                &format!("{tail}{go} Work, or {enter} on a card, to decide"),
                w,
            ),
            label_style(t),
        ))),
        Rect {
            y: inner.y + inner.height - 1,
            height: 1,
            ..inner
        },
    );
}

// ---------------------------------------------------------------------
// VERIFICATION
// ---------------------------------------------------------------------

/// First-run passes below this many judged jobs are called a small sample.
const SMALL_SAMPLE: u32 = 5;

fn hours_text(m: &MetricsView) -> String {
    let h = m.window_ms / 3_600_000;
    if h > 0 && h.is_multiple_of(24) && h / 24 > 1 {
        format!("last {}d", h / 24)
    } else if h == 24 {
        "today · 24h".to_string()
    } else {
        format!("last {h}h")
    }
}

fn verification(frame_: &mut Frame, area: Rect, app: &App, snap: &ConsoleSnapshot) {
    let t = &app.look.theme;
    let note = snap.metrics.as_ref().map(hours_text).unwrap_or_default();
    let Some(inner) = open(frame_, area, app, "VERIFICATION", &note, t.border) else {
        return;
    };
    let w = usize::from(inner.width);
    let key = 15;
    let value_w = w.saturating_sub(key + 1);
    let mut lines: Lines = Vec::new();
    let Some(m) = &snap.metrics else {
        lines.push(Line::from(span(
            "no ledger is read here: the figures are unknown",
            Style::new().fg(t.dim),
        )));
        frame_.render_widget(Paragraph::new(lines), inner);
        return;
    };
    let plain = |s: String| vec![span(s, Style::new().fg(t.text))];

    let per_hour = m
        .verified_per_hour
        .map(|r| format!(" · {r:.1}/h"))
        .unwrap_or_default();
    lines.push(row(
        t,
        "jobs verified",
        key,
        plain(format!("{}{per_hour}", m.verified)),
    ));
    lines.push(row(
        t,
        "first-run pass",
        key,
        match m.first_run {
            Some(f) => {
                // The count is always beside the percentage; a few judged
                // jobs say so.
                let small = if f.judged < SMALL_SAMPLE {
                    " · small n"
                } else {
                    ""
                };
                vec![
                    span(
                        format!("{:.0}%", f.rate * 100.0),
                        Style::new().fg(t.text).add_modifier(Modifier::BOLD),
                    ),
                    span(
                        fit(
                            &format!(" ({} of {}){small}", f.passed, f.judged),
                            value_w.saturating_sub(4),
                        ),
                        label_style(t),
                    ),
                ]
            }
            None => unknown(t, "no first claim judged in the window"),
        },
    ));
    lines.push(row(
        t,
        "claim→verified",
        key,
        match m.claim_to_verified {
            Some(s) => plain(fit(
                &format!(
                    "median {} · p90 {} · n={}",
                    span_text(s.median_ms as i64),
                    span_text(s.p90_ms as i64),
                    s.samples
                ),
                value_w,
            )),
            None => unknown(t, "no verified claim yet"),
        },
    ));
    lines.push(row(t, "checks run", key, plain(m.checks_run.to_string())));
    lines.push(row(
        t,
        "evidence",
        key,
        plain(format!(
            "{} bundle{} sealed",
            m.bundles_sealed,
            if m.bundles_sealed == 1 { "" } else { "s" }
        )),
    ));
    let escalated: Vec<&OutcomeView> = snap
        .groups
        .iter()
        .flat_map(|g| &g.outcomes)
        .filter(|o| o.lifecycle == Lifecycle::Escalated)
        .collect();
    lines.push(row(
        t,
        "escalated",
        key,
        match escalated.first() {
            None => vec![span("none open", Style::new().fg(t.ok))],
            Some(first) => vec![span(
                fit(
                    &format!("{} open · {}", escalated.len(), first.title),
                    value_w,
                ),
                Style::new().fg(t.error),
            )],
        },
    ));
    if !m.complete || m.covered_ms < m.window_ms {
        lines.push(Line::from(span(
            fit(
                &format!(
                    "the ledger covers {} of the window{}",
                    span_text(m.covered_ms as i64),
                    if m.complete { "" } else { " (read cut short)" }
                ),
                w,
            ),
            Style::new().fg(t.dim),
        )));
    }
    lines.truncate(usize::from(inner.height));
    frame_.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------
// LIMITS · BUDGET
// ---------------------------------------------------------------------

fn window<'a>(snap: &'a ConsoleSnapshot, name: &str) -> Option<&'a LimitWindow> {
    snap.header.limits.iter().find(|l| l.window == name)
}

fn limits(frame_: &mut Frame, area: Rect, app: &App, snap: &ConsoleSnapshot) {
    let t = &app.look.theme;
    let Some(inner) = open(frame_, area, app, "LIMITS · BUDGET", "", t.border) else {
        return;
    };
    let w = usize::from(inner.width);
    let now = app.now();
    let mut lines: Lines = Vec::new();

    let mut any = false;
    for (name, title) in [
        ("five_hour", "5-HOUR WINDOW"),
        ("seven_day", "7-DAY WINDOW"),
    ] {
        let Some(l) = window(snap, name) else {
            continue;
        };
        any = true;
        let ratio = l.utilization.unwrap_or(0.0).clamp(0.0, 1.0);
        let pct = l
            .utilization
            .map_or_else(|| "?".to_string(), |u| format!("{:.1}%", u * 100.0));
        let resets = l.resets_at.map_or_else(
            || "reset time unknown".to_string(),
            |r| format!("resets in {}", span_text(r - now)),
        );
        let stale = if l.stale { " · stale" } else { "" };
        let bar_w = w
            .saturating_sub(title.len() + 2)
            .clamp(6, 24)
            .min(w / 3)
            .max(6);
        let mut spans = vec![span(format!("{title:<14} "), label_style(t))];
        spans.extend(bar_spans(ratio, bar_w, &t.gauge, t.dim));
        spans.push(span(
            format!(" {pct}"),
            Style::new().fg(t.text).add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::from(spans));
        lines.push(Line::from(span(
            format!(
                "  {}",
                fit(&format!("{resets}{stale}"), w.saturating_sub(2))
            ),
            label_style(t),
        )));
        if let Some(h) = l.hits_at {
            let tri = if app.look.icons == IconSet::Ascii {
                "^"
            } else {
                "▲"
            };
            lines.push(Line::from(span(
                fit(
                    &format!(
                        "{tri} forecast: cap in {} at the current pace",
                        span_text(h - now)
                    ),
                    w,
                ),
                Style::new().fg(t.warn),
            )));
        }
    }
    if !any {
        lines.push(Line::from(span(
            "usage limits are not read yet",
            Style::new().fg(t.dim),
        )));
    }

    lines.push(Line::default());
    let s = &snap.header.spend;
    let money = |x: Option<f64>| x.map_or_else(|| "unknown".to_string(), usd);
    lines.push(row(
        t,
        "SPEND TODAY",
        14,
        vec![span(
            money(s.today_usd),
            Style::new().fg(t.text).add_modifier(Modifier::BOLD),
        )],
    ));
    lines.push(row(
        t,
        "THIS WEEK",
        14,
        vec![span(money(s.week_usd), Style::new().fg(t.text))],
    ));
    lines.push(row(
        t,
        "this run",
        14,
        vec![span(
            format!(
                "jobs {} · missions {}",
                usd(s.jobs_usd),
                usd(s.missions_usd)
            ),
            Style::new().fg(t.muted),
        )],
    ));
    // Spend by mission, where a mission has one: against its own budget.
    let by_mission: Vec<String> = snap
        .groups
        .iter()
        .flat_map(|g| &g.outcomes)
        .filter(|o| o.kind == OutcomeKind::Mission && o.spend.total_usd > 0.0)
        .map(|o| {
            let name = o.mission.as_deref().unwrap_or(&o.title);
            match o.contract.as_ref().and_then(|c| c.budget.usd) {
                Some(cap) => format!("{name} {}/{}", usd(o.spend.total_usd), usd(cap)),
                None => format!("{name} {}", usd(o.spend.total_usd)),
            }
        })
        .collect();
    if !by_mission.is_empty() {
        lines.push(row(
            t,
            "by mission",
            14,
            vec![span(
                fit(&by_mission.join(" · "), w.saturating_sub(15)),
                Style::new().fg(t.muted),
            )],
        ));
    }
    lines.truncate(usize::from(inner.height));
    frame_.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------
// GUARD EXCEPTIONS
// ---------------------------------------------------------------------

/// `1,288`: thousands grouped.
fn grouped(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The rule that decided a hold; a call held by risk alone matched no rule,
/// and the risk is the reason.
fn rule_words(h: &swarmux_application::console::HoldView) -> String {
    match (&h.rule, h.destructive) {
        (Some(rule), _) => rule.clone(),
        (None, true) => "destructive risk".into(),
        (None, false) => "no rule matched".into(),
    }
}

fn guard(frame_: &mut Frame, area: Rect, app: &App, snap: &ConsoleSnapshot) {
    let t = &app.look.theme;
    let recent: Vec<_> = app.engine.guard.recent().collect();
    let allowed = recent
        .iter()
        .filter(|d| d.outcome == Outcome::Allow)
        .count();
    let note = if recent.is_empty() {
        "nothing decided yet".to_string()
    } else {
        format!(
            "{} of the last {} calls were routine allows, not listed",
            grouped(allowed),
            grouped(recent.len())
        )
    };
    let Some(inner) = open(frame_, area, app, "GUARD EXCEPTIONS", &note, t.border) else {
        return;
    };
    let w = usize::from(inner.width);
    let offset = (app.utc_offset)();
    let mut lines: Lines = Vec::new();
    // The summary takes the last row, a rule above it.
    let summary_rows = usize::from(inner.height >= 6) * 2;
    let room = usize::from(inner.height).saturating_sub(summary_rows);
    if snap.holds.is_empty() {
        lines.push(Line::from(span(
            "no call is held or refused",
            Style::new().fg(t.ok),
        )));
    }
    // The rule and the answer are on the right, as wide as the longest.
    let rule_w = snap
        .holds
        .iter()
        .take(room)
        .map(|h| rule_words(h).chars().count())
        .max()
        .unwrap_or(0);
    let note_w = 18usize.min(w / 4);
    // The call keeps at least 32 cells; the rule gives way, not the call.
    let rule_w = rule_w.min(w.saturating_sub(22 + note_w + 32));
    for h in snap.holds.iter().take(room) {
        let held = h.state == HoldState::Held;
        let (word, color) = if held {
            ("HELD  ", t.warn)
        } else {
            ("DENIED", t.error)
        };
        let job = h
            .job
            .map_or_else(|| "    ".to_string(), |j| format!("#{j:<3}"));
        let call = format!("{} {}", h.tool, h.target);
        let fixed = 5 + 1 + 6 + 1 + 4 + 1 + 2 + rule_w + 2 + note_w;
        let call_w = w.saturating_sub(fixed);
        let rule_text = rule_words(h);
        lines.push(Line::from(vec![
            span(format!("{} ", local_clock(h.at, offset)), label_style(t)),
            span(
                format!("{word} "),
                Style::new().fg(color).add_modifier(Modifier::BOLD),
            ),
            span(format!("{job} "), Style::new().fg(t.label)),
            span(
                format!("{:<call_w$}  ", fit(&call, call_w)),
                Style::new().fg(t.text),
            ),
            span(
                format!("{:>rule_w$}  ", fit(&rule_text, rule_w)),
                Style::new().fg(if held { t.warn } else { t.error }),
            ),
            span(
                fit(
                    if held {
                        "decision in Work"
                    } else {
                        "no answer needed"
                    },
                    note_w,
                ),
                label_style(t),
            ),
        ]));
    }
    frame_.render_widget(Paragraph::new(lines), inner);
    if summary_rows > 0 {
        let base = inner.y + inner.height - 2;
        frame_.render_widget(
            Paragraph::new(Line::from(span(
                rule(app).to_string().repeat(w),
                Style::new().fg(t.dim),
            ))),
            Rect {
                y: base,
                height: 1,
                ..inner
            },
        );
        // Held and refused calls by the rule that decided them.
        let mut by_rule: std::collections::BTreeMap<String, (u32, u32)> =
            std::collections::BTreeMap::new();
        for h in &snap.holds {
            let e = by_rule
                .entry(h.rule.clone().unwrap_or_else(|| {
                    if h.destructive {
                        "destructive risk".into()
                    } else {
                        "no rule matched".into()
                    }
                }))
                .or_default();
            if h.state == HoldState::Held {
                e.0 += 1;
            } else {
                e.1 += 1;
            }
        }
        let text = if by_rule.is_empty() {
            "by rule: nothing held or refused".to_string()
        } else {
            format!(
                "by rule: {}",
                by_rule
                    .iter()
                    .map(|(r, (held, denied))| {
                        let mut parts = Vec::new();
                        if *held > 0 {
                            parts.push(format!("{held} held"));
                        }
                        if *denied > 0 {
                            parts.push(format!("{denied} denied"));
                        }
                        format!("{r} {}", parts.join(", "))
                    })
                    .collect::<Vec<_>>()
                    .join(" · ")
            )
        };
        frame_.render_widget(
            Paragraph::new(Line::from(span(fit(&text, w), label_style(t)))),
            Rect {
                y: base + 1,
                height: 1,
                ..inner
            },
        );
    }
}

// ---------------------------------------------------------------------
// TIMELINE
// ---------------------------------------------------------------------

fn timeline(frame_: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let changes = &app.mission.changes;
    let note = if changes.is_empty() {
        String::new()
    } else {
        "newest first".to_string()
    };
    let Some(inner) = open(frame_, area, app, "TIMELINE", &note, t.border) else {
        return;
    };
    let w = usize::from(inner.width);
    if changes.is_empty() {
        frame_.render_widget(
            Paragraph::new(Line::from(span(
                "nothing has happened yet",
                Style::new().fg(t.dim),
            ))),
            inner,
        );
        return;
    }
    let now = app.now();
    let ascii = app.look.icons == IconSet::Ascii;
    let lines: Lines = changes
        .iter()
        .take(usize::from(inner.height))
        .map(|c| {
            let (mark, color) = match (c.kind, ascii) {
                (ChangeKind::Failed | ChangeKind::Denied | ChangeKind::Stopped, false) => {
                    ("×", t.error)
                }
                (ChangeKind::Failed | ChangeKind::Denied | ChangeKind::Stopped, true) => {
                    ("x", t.error)
                }
                (ChangeKind::Finished | ChangeKind::Reviewed, false) => ("✓", t.ok),
                (ChangeKind::Finished | ChangeKind::Reviewed, true) => ("v", t.ok),
                (ChangeKind::Gate, false) if c.text.starts_with("gate passed") => ("✓", t.ok),
                (ChangeKind::Gate, true) if c.text.starts_with("gate passed") => ("v", t.ok),
                (ChangeKind::Gate | ChangeKind::Watchdog | ChangeKind::Asked, false) => {
                    ("◆", t.warn)
                }
                (ChangeKind::Gate | ChangeKind::Watchdog | ChangeKind::Asked, true) => {
                    ("*", t.warn)
                }
                (_, false) => ("·", t.muted),
                (_, true) => (".", t.muted),
            };
            let who = c
                .job
                .map_or_else(|| fit(&c.session, 10), |j| format!("#{}", j.0));
            Line::from(vec![
                span(format!("{:>4} ", age(now - c.at)), label_style(t)),
                span(format!("{mark} "), Style::new().fg(color)),
                span(format!("{who:<5} "), Style::new().fg(t.label)),
                span(fit(&c.text, w.saturating_sub(13)), Style::new().fg(t.text)),
            ])
        })
        .collect();
    frame_.render_widget(Paragraph::new(lines), inner);
}
