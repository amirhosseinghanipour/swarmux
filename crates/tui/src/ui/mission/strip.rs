//! The top strip: state counts, queue, burn, machine load, the usage
//! windows with their pace marker, and a tell for every stale feed.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_application::LimitView;
use swarmux_domain::Millis;
use swarmux_domain::telemetry::{short_span, window_length};

use super::{WIDE, glyph};
use crate::app::App;
use crate::mission::{TellKind, Tile, TileState};
use crate::theme::Theme;
use crate::ui::dashboard::{dim, fit, label};
use crate::ui::look::Icon;
use crate::ui::widgets::bar_spans;

/// Separator between strip segments.
const SEP: &str = " │ ";

pub fn render(frame: &mut Frame, area: Rect, app: &App, tiles: &[Tile], now: Millis) {
    let w = usize::from(area.width);
    let wide = area.width >= WIDE;
    let rows = if wide {
        vec![1, 1, 1]
    } else {
        vec![1, 1, 1, 1]
    };
    let areas = Layout::vertical(rows.iter().map(|h| Constraint::Length(*h))).split(area);
    let limits = &app.mission.snapshot.limits;
    let five = limits.iter().find(|l| l.window == "five_hour");
    let seven = limits.iter().find(|l| l.window == "seven_day");

    frame.render_widget(counts_line(app, tiles, w), areas[0]);
    if wide {
        let [a, b] = Layout::horizontal([Constraint::Percentage(50), Constraint::Fill(1)])
            .spacing(2)
            .areas(areas[1]);
        frame.render_widget(limit_line(app, "5h", five, usize::from(a.width)), a);
        frame.render_widget(limit_line(app, "7d", seven, usize::from(b.width)), b);
        frame.render_widget(info_line(app, now, w), areas[2]);
    } else {
        frame.render_widget(limit_line(app, "5h", five, w), areas[1]);
        frame.render_widget(limit_line(app, "7d", seven, w), areas[2]);
        frame.render_widget(info_line(app, now, w), areas[3]);
    }
}

/// `● run 2  ◐ wait 1  ✓ done 5  ✗ err 0 │ queue 3 │ burn $3.20/h │ cpu 37% mem 58%`,
/// segments dropped from the right when the row is narrow.
fn counts_line(app: &App, tiles: &[Tile], width: usize) -> Paragraph<'static> {
    let t = &app.look.theme;
    let count = |s: TileState| tiles.iter().filter(|x| x.state == s).count();
    let state = |icon: Icon, name: &str, n: usize, color| {
        vec![
            Span::styled(glyph(app, icon), Style::new().fg(color)),
            Span::styled(format!(" {name} "), Style::new().fg(t.label)),
            Span::styled(
                n.to_string(),
                Style::new()
                    .fg(if n == 0 { t.dim } else { t.text })
                    .add_modifier(Modifier::BOLD),
            ),
        ]
    };
    let mut states: Vec<Span> = Vec::new();
    for (i, seg) in [
        state(Icon::StateRun, "run", count(TileState::Running), t.ok),
        state(Icon::StateWait, "wait", count(TileState::Waiting), t.warn),
        state(Icon::StateDone, "done", count(TileState::Done), t.muted),
        state(Icon::StateErr, "err", count(TileState::Failed), t.error),
    ]
    .into_iter()
    .enumerate()
    {
        if i > 0 {
            states.push(Span::raw("  "));
        }
        states.extend(seg);
    }

    let stats = app.stats();
    let queue = vec![
        label(t, "queue "),
        Span::styled(stats.queued.to_string(), Style::new().fg(t.text)),
    ];
    let burn = app
        .mission
        .usage()
        .and_then(|u| u.burn)
        .map_or("—".to_string(), |b| format!("${:.2}/h", b.usd_per_hour));
    let burn = vec![
        label(t, "burn "),
        Span::styled(burn, Style::new().fg(t.text)),
    ];
    let machine = app.mission.snapshot.system.data.as_ref().map(|s| {
        let mem = if s.mem_total > 0 {
            s.mem_used as f64 / s.mem_total as f64 * 100.0
        } else {
            0.0
        };
        vec![
            label(t, "cpu "),
            Span::styled(format!("{:.0}%", s.cpu), Style::new().fg(t.text)),
            label(t, " mem "),
            Span::styled(format!("{mem:.0}%"), Style::new().fg(t.text)),
        ]
    });

    let mut spans = states;
    for seg in [Some(queue), Some(burn), machine].into_iter().flatten() {
        let next: usize = seg.iter().map(Span::width).sum::<usize>() + SEP.chars().count();
        if Line::from(spans.clone()).width() + next > width {
            break;
        }
        spans.push(dim(t, SEP));
        spans.extend(seg);
    }
    Paragraph::new(Line::from(spans))
}

/// `5h ▕█████┃─────▏ 42%  resets 2h 14m · hits cap ~16:40`
fn limit_line(app: &App, name: &str, view: Option<&LimitView>, width: usize) -> Paragraph<'static> {
    let t = &app.look.theme;
    let head = format!("{name} ");
    let Some(v) = view else {
        let note = format!("{head}no reading yet: limits appear once a job has run");
        return Paragraph::new(Line::from(dim(t, fit(&note, width))));
    };
    let ratio = v.utilization.unwrap_or(0.0);
    let value = format!(" {:>3.0}%", ratio * 100.0);
    let mut detail = String::new();
    if let Some(left) = v.resets_in {
        detail.push_str(&format!("  resets {}", short_span(left)));
    }
    if let Some(at) = &v.hits_at_clock {
        detail.push_str(&format!(" · hits cap ~{at}"));
    }
    if v.stale {
        detail.push_str(" · stale");
    }
    let fixed = head.chars().count() + value.chars().count();
    let room = width.saturating_sub(fixed);
    if room < detail.chars().count() + 8 {
        // Not enough room: the detail gives way, not the bar.
        detail = fit(&detail, room.saturating_sub(8));
    }
    let bar_w = room.saturating_sub(detail.chars().count()).min(24);
    let pace = pace_of(v);
    let mut spans = vec![label(t, head)];
    spans.extend(gauge_spans(app, ratio, bar_w, pace, v.stale));
    spans.push(Span::styled(
        value,
        Style::new().fg(if v.stale { t.dim } else { t.gauge_at(ratio) }),
    ));
    spans.push(Span::styled(detail, Style::new().fg(t.muted)));
    Paragraph::new(Line::from(spans))
}

/// How far through its window the clock is, `0..=1`.
fn pace_of(v: &LimitView) -> Option<f64> {
    let length = window_length(&v.window)?;
    let left = v.resets_in?;
    Some((1.0 - left as f64 / length as f64).clamp(0.0, 1.0))
}

/// The bar with the pace marker drawn over the cell the window's clock is at.
fn gauge_spans(
    app: &App,
    ratio: f64,
    width: usize,
    pace: Option<f64>,
    stale: bool,
) -> Vec<Span<'static>> {
    let t: &Theme = &app.look.theme;
    if width == 0 {
        return Vec::new();
    }
    let stops = if stale { [t.dim; 3] } else { t.gauge };
    let mut cells = bar_spans(ratio, width, &stops, t.dim);
    if let Some(p) = pace {
        let at = ((p * width as f64) as usize).min(width - 1);
        cells[at] = Span::styled(
            glyph(app, Icon::Pace),
            Style::new().fg(t.text).add_modifier(Modifier::BOLD),
        );
    }
    let mut spans = vec![dim(t, "▕")];
    spans.extend(cells);
    spans.push(dim(t, "▏"));
    spans
}

/// Staleness tells first, then the plain facts of today's spend.
fn info_line(app: &App, now: Millis, width: usize) -> Paragraph<'static> {
    let t = &app.look.theme;
    let mut spans: Vec<Span<'static>> = Vec::new();
    for tell in app.mission.tells(now) {
        let (text, color) = match tell.kind {
            TellKind::Old(age) => (format!("{} {} old", tell.feed, super::age(age)), t.warn),
            TellKind::Failed => (format!("{} unreadable", tell.feed), t.error),
        };
        spans.push(Span::styled(
            format!("{} {text}", glyph(app, Icon::Stalled)),
            Style::new().fg(color),
        ));
        spans.push(Span::raw("  "));
    }
    if let Some(u) = app.mission.usage() {
        let cache = u
            .cache_hit_ratio_today
            .map_or(String::new(), |c| format!(" · cache hit {:.0}%", c * 100.0));
        spans.push(dim(
            t,
            format!(
                "today ${:.2} · {} live session{}{cache}",
                u.today.cost,
                u.live_sessions,
                if u.live_sessions == 1 { "" } else { "s" }
            ),
        ));
    } else if spans.is_empty() {
        spans.push(dim(
            t,
            "spend appears once Claude Code transcripts are read",
        ));
    }
    let line = Line::from(spans);
    if line.width() > width {
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        return Paragraph::new(Line::from(dim(t, fit(&text, width))));
    }
    Paragraph::new(line)
}
