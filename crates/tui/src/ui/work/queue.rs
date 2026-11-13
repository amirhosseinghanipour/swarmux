//! The left column: the ranked queue (DECIDE, READY TO LAND, RUNNING,
//! LANDED) and the capacity of the usage windows below it.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_application::console::{
    ConsoleSnapshot, GroupKind, GroupView, HoldState, LimitWindow, OutcomeView,
};
use swarmux_domain::Millis;

use super::util::{
    Lines, Mark, due_text, duration, fit, heading, lifecycle_word, mark, priority_color,
    priority_text, span,
};
use crate::app::App;
use crate::theme::Theme;
use crate::ui::focus::parts::meter;
use crate::ui::mission::age;
use crate::work::{countdown, group_note, group_title, holds_of, relations, short};

/// Rows the capacity footer needs below which it is left out.
const FOOTER_FROM_HEIGHT: u16 = 22;

/// One outcome's rows, and whether it is the selected one.
struct Entry {
    lines: Lines,
    selected: bool,
}

pub(super) fn render(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    snap: &ConsoleSnapshot,
    selected: Option<&str>,
) {
    let w = usize::from(area.width);
    let footer = if area.height >= FOOTER_FROM_HEIGHT {
        capacity(app, snap, w)
    } else {
        Vec::new()
    };
    let footer_h = u16::try_from(footer.len()).unwrap_or(0);
    let list_h = usize::from(
        area.height
            .saturating_sub(footer_h + u16::from(footer_h > 0)),
    );

    // With room, outcomes breathe; without, they sit on consecutive rows.
    let roomy = build(app, snap, selected, w, true);
    let total: usize = roomy.iter().map(|e| e.lines.len()).sum();
    let entries = if total <= list_h {
        roomy
    } else {
        build(app, snap, selected, w, false)
    };

    // Keep the selected outcome in view: scroll so its last row shows.
    let mut at = 0;
    let mut end_of_selected = 0;
    for e in &entries {
        at += e.lines.len();
        if e.selected {
            end_of_selected = at;
        }
    }
    let skip = end_of_selected.saturating_sub(list_h);
    let lines: Lines = entries
        .into_iter()
        .flat_map(|e| e.lines)
        .skip(skip)
        .take(list_h)
        .collect();
    let list = Rect {
        height: u16::try_from(list_h).unwrap_or(area.height),
        ..area
    };
    frame.render_widget(Paragraph::new(lines), list);
    if footer_h > 0 {
        let at = Rect {
            y: area.y + area.height - footer_h,
            height: footer_h,
            ..area
        };
        frame.render_widget(Paragraph::new(footer), at);
    }
}

fn build(
    app: &App,
    snap: &ConsoleSnapshot,
    selected: Option<&str>,
    w: usize,
    roomy: bool,
) -> Vec<Entry> {
    let t = &app.look.theme;
    let now = app.now();
    let mut out: Vec<Entry> = Vec::new();
    let shown: usize = snap.groups.iter().map(|g| g.total).sum();
    if shown == 0 {
        out.push(Entry {
            lines: vec![
                heading(app, "WORK", "", w, t.muted),
                Line::from(span(
                    "nothing needs you, nothing is ready to land",
                    Style::new().fg(t.muted),
                )),
                Line::from(span(
                    "start a mission or a prompt: it shows here",
                    Style::new().fg(t.dim),
                )),
            ],
            selected: false,
        });
        return out;
    }
    for g in &snap.groups {
        if g.outcomes.is_empty() && g.kind != GroupKind::Decide {
            continue;
        }
        let color = group_color(t, g.kind);
        let mut head = vec![heading(
            app,
            &format!("{} {}", group_title(g.kind), g.total),
            group_note(g.kind),
            w,
            color,
        )];
        if g.outcomes.is_empty() {
            head.push(Line::from(span(
                "  nothing needs you",
                Style::new().fg(t.muted),
            )));
        }
        out.push(Entry {
            lines: head,
            selected: false,
        });
        for (i, o) in g.outcomes.iter().enumerate() {
            let sel = selected == Some(o.id.as_str());
            let mut lines = item(app, snap, g, o, now, w, sel);
            if roomy && g.kind != GroupKind::Landed && i + 1 < g.outcomes.len() {
                lines.push(Line::default());
            }
            out.push(Entry {
                lines,
                selected: sel,
            });
        }
        if g.total > g.outcomes.len() {
            out.push(Entry {
                lines: vec![Line::from(span(
                    format!("  +{} earlier", g.total - g.outcomes.len()),
                    Style::new().fg(t.muted),
                ))],
                selected: false,
            });
        }
        if roomy {
            out.push(Entry {
                lines: vec![Line::default()],
                selected: false,
            });
        }
    }
    out
}

pub(super) fn group_color(t: &Theme, kind: GroupKind) -> ratatui::style::Color {
    match kind {
        GroupKind::Decide => t.warn,
        GroupKind::ReadyToLand => t.ok,
        GroupKind::Running => t.label,
        GroupKind::Landed => t.muted,
    }
}

/// The folder an outcome works in, or the repositories its contract names.
fn repo_of(o: &OutcomeView) -> String {
    if let Some(j) = o.jobs.first() {
        return j.repo.clone();
    }
    o.contract
        .as_ref()
        .map(|c| c.repos.join(", "))
        .unwrap_or_default()
}

/// The rows of one outcome. The selected one has a bar at its left and a
/// bold title.
fn item(
    app: &App,
    snap: &ConsoleSnapshot,
    g: &GroupView,
    o: &OutcomeView,
    now: Millis,
    w: usize,
    sel: bool,
) -> Lines {
    let t = &app.look.theme;
    let holds = holds_of(snap, o);
    let held = holds.iter().find(|h| h.state == HoldState::Held);
    let (glyph, color) = match g.kind {
        GroupKind::Decide => {
            let failed = matches!(
                o.lifecycle,
                swarmux_application::console::Lifecycle::Failed
                    | swarmux_application::console::Lifecycle::Escalated
                    | swarmux_application::console::Lifecycle::Interrupted
            );
            let m = if held.is_none() && failed {
                Mark::Bad
            } else {
                Mark::Ask
            };
            (m, o.priority.map_or(t.warn, |p| priority_color(t, p)))
        }
        GroupKind::ReadyToLand => (Mark::Ready, t.ok),
        GroupKind::Running => (Mark::Open, t.label),
        GroupKind::Landed => (Mark::Pass, t.ok),
    };
    let title_style = if sel {
        Style::new().fg(t.title).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(t.text)
    };
    let muted = Style::new().fg(t.muted);

    // First row: mark, title, and on the right the priority or the time.
    let right = match g.kind {
        GroupKind::Decide => o.priority.map(|p| priority_text(p).to_string()),
        GroupKind::Landed => Some(landed_right(o, now)),
        _ => None,
    };
    let right_w = right.as_deref().map_or(0, |r| r.chars().count() + 1);
    let title_w = w.saturating_sub(4 + right_w);
    let mut first = vec![
        Span::styled(if sel { "▌ " } else { "  " }, Style::new().fg(t.warn)),
        Span::styled(format!("{} ", mark(app, glyph)), Style::new().fg(color)),
        Span::styled(fit(&o.title, title_w), title_style),
    ];
    if let Some(r) = right {
        let used = Line::from(first.clone()).width();
        let pad = w.saturating_sub(used + r.chars().count());
        first.push(Span::raw(" ".repeat(pad)));
        let style = match (g.kind, o.priority) {
            (GroupKind::Decide, Some(p)) => Style::new()
                .fg(priority_color(t, p))
                .add_modifier(Modifier::BOLD),
            _ => muted,
        };
        first.push(Span::styled(r, style));
    }
    let mut rows = vec![Line::from(first)];

    if g.kind != GroupKind::Landed {
        let detail = detail(snap, g.kind, o, held.copied(), now);
        rows.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(fit(&detail, w.saturating_sub(4)), muted),
        ]));
    }
    if g.kind == GroupKind::Decide
        && let Some(cost) = cost_line(snap, o, now, (app.utc_offset)())
    {
        rows.push(Line::from(vec![
            Span::raw("    "),
            Span::styled("cost ", Style::new().fg(t.dim)),
            Span::styled(fit(&cost, w.saturating_sub(9)), Style::new().fg(t.warn)),
        ]));
    }
    rows
}

/// What the second row says.
fn detail(
    snap: &ConsoleSnapshot,
    kind: GroupKind,
    o: &OutcomeView,
    held: Option<&swarmux_application::console::HoldView>,
    now: Millis,
) -> String {
    let repo = repo_of(o);
    let lead = |text: String| {
        if repo.is_empty() {
            text
        } else {
            format!("{repo} · {text}")
        }
    };
    match kind {
        GroupKind::Decide => {
            let why = match held {
                Some(h) => format!(
                    "{} held · {}",
                    h.rule.as_deref().unwrap_or(h.tool.as_str()),
                    age(now - h.at)
                ),
                None => o.asks.iter().find(|a| a.decides).map_or_else(
                    || lifecycle_word(o.lifecycle).to_string(),
                    |a| a.title.clone(),
                ),
            };
            lead(why)
        }
        GroupKind::ReadyToLand => {
            let checks = if o.checks.total > 0 {
                format!("{}/{} checks", o.checks.passed, o.checks.total)
            } else {
                "gate passed".to_string()
            };
            lead(format!("{checks} · waits {}", age(now - o.since_at)))
        }
        GroupKind::Running => {
            let ctx = o
                .jobs
                .iter()
                .find_map(|j| j.context_use)
                .map(|c| format!(" · ctx {:.0}%", c * 100.0))
                .unwrap_or_default();
            let blocked = relations(snap, o)
                .waits_on
                .first()
                .map(|r| format!(" · waits on {}", r.title))
                .unwrap_or_default();
            lead(format!("{}{ctx}{blocked}", lifecycle_word(o.lifecycle)))
        }
        GroupKind::Landed => String::new(),
    }
}

/// The cost of waiting: only what the data says. The countdown of the
/// auto-deny of a held call, and what this holds up through the task
/// dependencies of its plan.
fn cost_line(
    snap: &ConsoleSnapshot,
    o: &OutcomeView,
    now: Millis,
    utc_offset: i32,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    // Only a contract that says when it is needed has a due time to show:
    // none is ever made up.
    if let Some(due) = o.contract.as_ref().and_then(|c| c.due) {
        parts.push(due_text(due, now, utc_offset));
    }
    let deadline = o
        .asks
        .iter()
        .filter(|a| a.decides)
        .filter_map(|a| a.deadline_at)
        .min();
    if let Some(d) = deadline {
        parts.push(format!("auto-deny in {}", countdown(d, now)));
    }
    // What waits on this one through the plan's task dependencies: a file
    // claim or a contract link is a relation, not a cost of delay.
    let rel = relations(snap, o);
    let blocked: Vec<_> = rel
        .blocks
        .iter()
        .filter(|r| r.kind == swarmux_application::console::EdgeKind::TaskDependency)
        .collect();
    if let Some(first) = blocked.first() {
        let extra = match blocked.len() - 1 {
            0 => String::new(),
            more => format!(" +{more}"),
        };
        parts.push(format!(
            "blocks {}{extra} ({})",
            first.title,
            crate::work::edge_label(first.kind)
        ));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// `12m ago · bundle 7c1e` for a landed outcome.
fn landed_right(o: &OutcomeView, now: Millis) -> String {
    let proof = o
        .evidence
        .as_ref()
        .map(|e| {
            let h = short(&e.bundle_hash);
            format!("bundle {}", h.get(..4).unwrap_or(h))
        })
        .or_else(|| {
            o.landing
                .as_ref()
                .and_then(|l| l.repos.iter().find_map(|r| r.commit.clone()))
                .map(|c| c.chars().take(7).collect())
        });
    match proof {
        Some(p) => format!("{} ago · {p}", age(now - o.since_at)),
        None => format!("{} ago", age(now - o.since_at)),
    }
}

/// The usage windows with their forecast, and what the scheduler holds.
fn capacity(app: &App, snap: &ConsoleSnapshot, w: usize) -> Lines {
    let t = &app.look.theme;
    let now = app.now();
    let mut lines = vec![heading(app, "CAPACITY", "", w, t.muted)];
    let windows: Vec<(&str, &LimitWindow)> = ["five_hour", "seven_day"]
        .into_iter()
        .zip(["5h", "7d"])
        .filter_map(|(name, label)| {
            snap.header
                .limits
                .iter()
                .find(|l| l.window == name)
                .map(|l| (label, l))
        })
        .collect();
    if windows.is_empty() {
        lines.push(Line::from(span(
            "usage limits are not read yet",
            Style::new().fg(t.muted),
        )));
    }
    for (label, l) in windows {
        let use_ = l.utilization.unwrap_or(0.0).clamp(0.0, 1.0);
        let color = if use_ >= 0.85 {
            t.gauge[2]
        } else if use_ >= 0.6 {
            t.gauge[1]
        } else {
            t.gauge[0]
        };
        let filled = (use_ * 8.0).round() as usize;
        let mut spans = vec![
            span(format!("{label} "), Style::new().fg(t.muted)),
            span(meter(app, filled, 8), Style::new().fg(color)),
            span(
                match l.utilization {
                    Some(u) => format!(" {:.0}%", u * 100.0),
                    None => " ?".to_string(),
                },
                Style::new().fg(color),
            ),
        ];
        let mut notes: Vec<String> = Vec::new();
        if let Some(h) = l.hits_at {
            notes.push(format!("cap in {}", age(h - now)));
        }
        if let Some(r) = l.resets_at {
            notes.push(format!("reset {}", age(r - now)));
        }
        if l.stale {
            notes.push("stale".to_string());
        }
        if !notes.is_empty() {
            spans.push(span(
                format!(" · {}", notes.join(" · ")),
                Style::new().fg(t.muted),
            ));
        }
        lines.push(Line::from(spans));
    }
    let q = &snap.header.queue;
    let mut sched = format!(
        "slots {}/{} · queued {}",
        q.running, q.max_concurrent, q.queued
    );
    if q.deferred > 0 {
        sched.push_str(&format!(" · deferred {}", q.deferred));
    }
    if q.awaiting_reset > 0 {
        sched.push_str(&format!(" · awaiting reset {}", q.awaiting_reset));
    }
    lines.push(Line::from(span(sched, Style::new().fg(t.dim))));
    lines.extend(metrics(app, snap, w));
    lines
}

/// How verification has gone over the metrics window, from the ledger:
/// what the gate passed at the first claim, how long a claim took to be
/// verified, how much was verified an hour, the checks that ran and the
/// bundles sealed. A figure with nothing behind it is left out, not shown
/// as zero.
fn metrics(app: &App, snap: &ConsoleSnapshot, w: usize) -> Lines {
    let t = &app.look.theme;
    let Some(m) = &snap.metrics else {
        return Vec::new();
    };
    let hours = (m.window_ms / 3_600_000).max(1);
    let mut head: Vec<String> = Vec::new();
    if let Some(f) = &m.first_run {
        head.push(format!(
            "first-run {:.0}% ({}/{})",
            f.rate * 100.0,
            f.passed,
            f.judged
        ));
    }
    if m.verified > 0 {
        head.push(format!("{} verified", m.verified));
    }
    if let Some(rate) = m.verified_per_hour {
        head.push(if rate < 0.1 {
            format!("{rate:.2}/h")
        } else {
            format!("{rate:.1}/h")
        });
    }
    // One figure group per row: the passes, how long a claim took, what ran.
    let mut groups: Vec<String> = Vec::new();
    if !head.is_empty() {
        groups.push(head.join(" · "));
    }
    if let Some(s) = &m.claim_to_verified {
        groups.push(format!(
            "claim→verified {} med · {} p90",
            duration(s.median_ms),
            duration(s.p90_ms)
        ));
    }
    let mut ran: Vec<String> = Vec::new();
    if m.checks_run > 0 {
        ran.push(format!("{} checks", m.checks_run));
    }
    if m.bundles_sealed > 0 {
        ran.push(format!("{} sealed", m.bundles_sealed));
    }
    if !m.complete {
        ran.push("newest only".to_string());
    }
    if !ran.is_empty() {
        groups.push(ran.join(" · "));
    }
    if groups.is_empty() {
        groups.push("nothing judged yet".to_string());
    }
    let label = format!("last {hours}h ");
    let pad = " ".repeat(label.chars().count());
    groups
        .into_iter()
        .enumerate()
        .map(|(i, text)| {
            let lead = if i == 0 { label.clone() } else { pad.clone() };
            let room = w.saturating_sub(lead.chars().count());
            Line::from(vec![
                span(lead, Style::new().fg(t.dim)),
                span(fit(&text, room), Style::new().fg(t.muted)),
            ])
        })
        .collect()
}
