//! The second layer of Mission control: real-data panels that only the
//! larger tiers have room for. Every one reads the telemetry snapshot or the
//! job services and shows a calm hint when there is nothing to read yet.
//! None of them animate.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_application::Collected;
use swarmux_domain::telemetry::{AgentNode, TodoStatus};
use swarmux_domain::{JobId, Millis};

use super::layout::Panel;
use super::{glyph, panel};
use crate::app::App;
use crate::theme::Theme;
use crate::ui::dashboard::{dim, fit, label};
use crate::ui::look::Icon;
use crate::ui::widgets::{Heatmap, Ring, SparkStyle, Sparkline, bar_spans};

pub fn render(frame: &mut Frame, area: Rect, app: &App, which: Panel, now: Millis) {
    let t = &app.look.theme;
    let (icon, title) = match which {
        Panel::Flow => (Icon::Tokens, "token flow"),
        Panel::Burn => (Icon::Burn, "burn"),
        Panel::Mix => (Icon::Mix, "model mix"),
        Panel::Agents => (Icon::Agents, "agents"),
        Panel::Tools => (Icon::Tools, "tools"),
        Panel::Files => (Icon::Files, "files"),
        Panel::Git => (Icon::Git, "git"),
        Panel::Ci => (Icon::Ci, "pull requests"),
        Panel::Mcp => (Icon::Env, "mcp"),
        Panel::System => (Icon::System, "system"),
        Panel::Calendar => (Icon::Calendar, "12 weeks"),
        _ => return,
    };
    let block = panel(t, glyph(app, icon), title, meta(app, which, now), t.border);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 8 || inner.height == 0 {
        return;
    }
    match which {
        Panel::Flow => flow(frame, inner, app),
        Panel::Burn => burn(frame, inner, app, now),
        Panel::Mix => mix(frame, inner, app),
        Panel::Agents => agents(frame, inner, app),
        Panel::Tools => tools(frame, inner, app),
        Panel::Files => files(frame, inner, app),
        Panel::Git => git(frame, inner, app),
        Panel::Ci => ci(frame, inner, app),
        Panel::Mcp => mcp(frame, inner, app),
        Panel::System => system(frame, inner, app),
        Panel::Calendar => calendar(frame, inner, app),
        _ => {}
    }
}

/// The right-hand title of a panel: how old its data is, when it is old.
fn meta(app: &App, which: Panel, now: Millis) -> Line<'static> {
    let t = &app.look.theme;
    let s = &app.mission.snapshot;
    let c = &app.mission.cadence;
    let age = |updated: Option<Millis>, every: Millis| -> Option<String> {
        updated
            .filter(|at| now - at > every * 2)
            .map(|at| format!(" {} old ", super::age(now - at)))
    };
    let text = match which {
        Panel::Burn | Panel::Mix | Panel::Calendar => age(s.usage.updated_at, c.usage),
        Panel::System => age(s.system.updated_at, c.system),
        Panel::Git => age(s.git.updated_at, c.git),
        Panel::Ci => age(s.github.updated_at, c.github),
        _ => None,
    };
    Line::from(Span::styled(
        text.unwrap_or_default(),
        Style::new().fg(t.warn),
    ))
}

fn hint(frame: &mut Frame, area: Rect, app: &App, text: &str) {
    let t = &app.look.theme;
    frame.render_widget(
        Line::from(dim(t, fit(text, usize::from(area.width)))),
        Rect { height: 1, ..area },
    );
}

fn row(area: Rect, n: u16) -> Rect {
    Rect {
        y: area.y + n,
        height: 1,
        ..area
    }
}

fn lines(frame: &mut Frame, area: Rect, lines: Vec<Line<'static>>) {
    let n = usize::from(area.height);
    frame.render_widget(
        Paragraph::new(lines.into_iter().take(n).collect::<Vec<_>>()),
        area,
    );
}

fn feed_note<T>(c: &Collected<T>, what: &str) -> String {
    match (&c.error, c.enabled) {
        (_, false) => format!("{what} is off in [telemetry]"),
        (Some(e), _) => format!("{what}: {e}"),
        _ => format!("waiting for {what}"),
    }
}

// --- token flow -----------------------------------------------------------

fn flow(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let a = &app.mission.snapshot.activity;
    if a.flow.is_empty() {
        return hint(frame, area, app, "no swarmux jobs have run yet");
    }
    let series = [
        (
            "in",
            a.flow.iter().map(|b| b.input as f64).collect::<Vec<_>>(),
        ),
        ("out", a.flow.iter().map(|b| b.output as f64).collect()),
        (
            "cache",
            a.flow.iter().map(|b| b.cache_read as f64).collect(),
        ),
    ];
    let w = usize::from(area.width);
    let label_w = 6u16;
    for (i, (name, data)) in series.iter().enumerate() {
        if i as u16 >= area.height {
            break;
        }
        let y = area.y + i as u16;
        let peak = data.iter().copied().fold(0.0, f64::max);
        frame.render_widget(
            Line::from(vec![
                label(t, format!("{name:<5}")),
                dim(t, format!("{:>6}", super::compact_count(peak))),
            ]),
            Rect::new(area.x, y, (label_w + 6).min(area.width), 1),
        );
        if w > usize::from(label_w) + 8 {
            frame.render_widget(
                Sparkline::new(data, &t.chart).style(SparkStyle::Block),
                Rect::new(area.x + label_w + 7, y, area.width - label_w - 7, 1),
            );
        }
    }
    if area.height > 3
        && let Some(ratio) = a.cache_hit_ratio
    {
        frame.render_widget(
            Line::from(dim(
                t,
                format!(
                    "cache hit {:.0}% · tokens per minute, last hour",
                    ratio * 100.0
                ),
            )),
            row(area, 3),
        );
    }
}

// --- burn ---------------------------------------------------------------

fn burn(frame: &mut Frame, area: Rect, app: &App, now: Millis) {
    let t = &app.look.theme;
    let Some(u) = app.mission.usage() else {
        return hint(
            frame,
            area,
            app,
            &feed_note(&app.mission.snapshot.usage, "transcripts"),
        );
    };
    let day: f64 = u
        .blocks
        .iter()
        .filter(|b| b.end > now - 24 * 3_600_000)
        .map(|b| b.cost)
        .sum();
    let rate = u
        .burn
        .map_or("—".to_string(), |b| format!("${:.2}/h", b.usd_per_hour));
    frame.render_widget(
        Line::from(vec![
            label(t, "24h "),
            Span::styled(
                format!("${day:.2}"),
                Style::new().fg(t.text).add_modifier(Modifier::BOLD),
            ),
            dim(t, format!("  today ${:.2}  now {rate}", u.today.cost)),
        ]),
        row(area, 0),
    );
    if area.height > 2 {
        let data: Vec<f64> = u.flow.iter().map(|b| b.total() as f64).collect();
        let chart = Rect {
            y: area.y + 1,
            height: area.height - 2,
            ..area
        };
        if data.is_empty() {
            hint(frame, chart, app, "no messages in the last hour");
        } else {
            frame.render_widget(
                Sparkline::new(&data, &t.chart)
                    .style(SparkStyle::Braille)
                    .baseline(true),
                chart,
            );
        }
        frame.render_widget(
            Line::from(dim(t, "tokens per minute, last hour")),
            row(area, area.height - 1),
        );
    }
}

// --- model mix and fetch ----------------------------------------------------

fn mix(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let snap = &app.mission.snapshot;
    let fetch_h = u16::from(area.height >= 4);
    let body = Rect {
        height: area.height - fetch_h,
        ..area
    };
    match app
        .mission
        .usage()
        .map(|u| &u.models_today)
        .filter(|m| !m.is_empty())
    {
        None => hint(frame, body, app, "no priced messages today"),
        Some(models) => {
            let shares: Vec<(&str, f64, ratatui::style::Color)> = models
                .iter()
                .take(5)
                .map(|m| (m.name.as_str(), m.share, t.model(Some(&m.name))))
                .collect();
            let total: f64 = models.iter().map(|m| m.spend.cost).sum();
            let center = format!("${total:.2}");
            frame.render_widget(
                Ring::new(&shares)
                    .center(&center)
                    .legend(true)
                    .track(t.dim)
                    .text(t.text),
                body,
            );
        }
    }
    if fetch_h == 1 {
        let claude = snap
            .environments
            .first()
            .and_then(|e| e.environment.claude_code_version.clone())
            .map_or("claude ?".to_string(), |v| format!("claude {v}"));
        frame.render_widget(
            Line::from(vec![
                Span::styled("swarmux ", Style::new().fg(t.label)),
                Span::styled(env!("CARGO_PKG_VERSION"), Style::new().fg(t.text)),
                dim(t, format!(" · {claude} · {}", std::env::consts::OS)),
            ]),
            row(area, area.height - 1),
        );
    }
}

// --- agents -----------------------------------------------------------------

/// The job the second layer follows: the selected inbox item's, else the
/// job the dashboard follows.
pub(super) fn followed_job(app: &App) -> Option<JobId> {
    app.mission
        .selected_item()
        .and_then(|i| i.job)
        .or_else(|| app.current_job().map(|j| j.job_id))
}

fn agents(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let Some(tree) = followed_job(app).and_then(|j| app.jobs.agent_tree(j)) else {
        return hint(frame, area, app, "select an item to see its agents");
    };
    let mut out = Vec::new();
    tree_lines(app, t, &tree, 0, usize::from(area.width), &mut out);
    lines(frame, area, out);
}

fn tree_lines(
    app: &App,
    t: &Theme,
    node: &AgentNode,
    depth: usize,
    width: usize,
    out: &mut Vec<Line<'static>>,
) {
    let mark = if node.running {
        Icon::StateRun
    } else {
        Icon::StateDone
    };
    let color = if node.running { t.ok } else { t.muted };
    let what = node
        .current_tool
        .as_deref()
        .map_or(String::new(), |c| format!(" · {c}"));
    let text = format!(
        "{}{}{} {} calls{what}",
        "  ".repeat(depth),
        "",
        node.label,
        node.tool_calls
    );
    out.push(Line::from(vec![
        Span::styled(glyph(app, mark), Style::new().fg(color)),
        Span::raw(" "),
        Span::styled(
            fit(&text, width.saturating_sub(2)),
            Style::new().fg(t.model(node.model.as_deref())),
        ),
    ]));
    for child in &node.children {
        tree_lines(app, t, child, depth + 1, width, out);
    }
}

// --- tools, files -------------------------------------------------------------

fn tools(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let a = &app.mission.snapshot.activity;
    if a.tools.is_empty() {
        return hint(frame, area, app, "no tool calls yet");
    }
    let peak = a.tools.iter().map(|s| s.calls).max().unwrap_or(1).max(1);
    let stats_h = (area.height / 2).max(1).min(a.tools.len() as u16);
    let w = usize::from(area.width);
    let bar_w = w.saturating_sub(18).clamp(3, 14);
    let mut out = Vec::new();
    for s in a.tools.iter().take(usize::from(stats_h)) {
        let mut spans = vec![Span::styled(
            format!("{:<8} ", fit(&s.name, 8)),
            Style::new().fg(t.label),
        )];
        spans.extend(bar_spans(
            f64::from(s.calls) / f64::from(peak),
            bar_w,
            &t.chart,
            t.dim,
        ));
        spans.push(Span::styled(
            format!(" {:>3}", s.calls),
            Style::new().fg(t.text),
        ));
        if s.errors > 0 {
            spans.push(Span::styled(
                format!(" !{}", s.errors),
                Style::new().fg(t.error),
            ));
        }
        out.push(Line::from(spans));
    }
    let room = usize::from(area.height).saturating_sub(out.len());
    for c in a.ticker.iter().take(room) {
        let color = if c.is_error { t.error } else { t.muted };
        out.push(Line::from(Span::styled(
            fit(&format!("{} {}", c.name, c.summary), w),
            Style::new().fg(color),
        )));
    }
    lines(frame, area, out);
}

fn files(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let a = &app.mission.snapshot.activity;
    if a.files.is_empty() {
        return hint(frame, area, app, "no files touched yet");
    }
    let w = usize::from(area.width);
    let out = a
        .files
        .iter()
        .map(|f| {
            let counts = format!(" r{} e{}", f.reads, f.edits);
            let name = tail(&f.path, w.saturating_sub(counts.chars().count()));
            Line::from(vec![
                Span::styled(name, Style::new().fg(t.text)),
                Span::styled(counts, Style::new().fg(t.muted)),
            ])
        })
        .collect();
    lines(frame, area, out);
}

/// The end of a path, which is the informative part.
fn tail(path: &str, max: usize) -> String {
    let n = path.chars().count();
    if n <= max || max < 2 {
        return path.chars().take(max).collect();
    }
    let keep: String = path.chars().skip(n - (max - 1)).collect();
    format!("…{keep}")
}

// --- git, pull requests -------------------------------------------------------

fn git(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let Some(g) = app.mission.snapshot.git.data.as_ref() else {
        return hint(
            frame,
            area,
            app,
            &feed_note(&app.mission.snapshot.git, "git"),
        );
    };
    let mut out = vec![Line::from(vec![
        Span::styled(
            g.branch.clone().unwrap_or_else(|| "detached".into()),
            Style::new().fg(t.title).add_modifier(Modifier::BOLD),
        ),
        dim(t, format!("  ↑{} ↓{}", g.ahead, g.behind)),
    ])];
    out.push(Line::from(dim(
        t,
        format!(
            "{} staged · {} changed · {} new · +{} −{}",
            g.staged, g.unstaged, g.untracked, g.diff.insertions, g.diff.deletions
        ),
    )));
    for c in &g.commits {
        out.push(Line::from(vec![
            Span::styled(format!("{} ", fit(&c.hash, 7)), Style::new().fg(t.label)),
            Span::styled(
                fit(&c.subject, usize::from(area.width).saturating_sub(8)),
                Style::new().fg(t.muted),
            ),
        ]));
    }
    lines(frame, area, out);
}

fn ci(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let Some(g) = app.mission.snapshot.github.data.as_ref() else {
        return hint(
            frame,
            area,
            app,
            &feed_note(&app.mission.snapshot.github, "gh"),
        );
    };
    let mut out = Vec::new();
    if let Some(run) = &g.ci {
        let (icon, color) = match run.conclusion.as_deref() {
            Some("success") => (Icon::Result, t.ok),
            Some(_) => (Icon::Failed, t.error),
            None => (Icon::StateWait, t.warn),
        };
        let state = run.conclusion.clone().unwrap_or_else(|| run.status.clone());
        out.push(Line::from(vec![
            Span::styled(glyph(app, icon), Style::new().fg(color)),
            Span::styled(
                format!(" {} {state}", fit(&run.workflow, 16)),
                Style::new().fg(t.text),
            ),
        ]));
    }
    if g.pull_requests.is_empty() {
        out.push(Line::from(dim(t, "no open pull requests")));
    }
    for p in &g.pull_requests {
        let review = p
            .review_decision
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .replace('_', " ");
        out.push(Line::from(vec![
            Span::styled(format!("#{} ", p.number), Style::new().fg(t.label)),
            Span::styled(
                fit(
                    &p.title,
                    usize::from(area.width).saturating_sub(12 + review.len()),
                ),
                Style::new().fg(t.text),
            ),
            dim(t, format!(" {review}")),
        ]));
    }
    lines(frame, area, out);
}

// --- mcp, system, calendar ------------------------------------------------------

fn mcp(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let Some(env) = app.mission.snapshot.environments.first() else {
        return hint(
            frame,
            area,
            app,
            "servers show once a job reports its tools",
        );
    };
    let servers = &env.environment.mcp_servers;
    if servers.is_empty() {
        return hint(frame, area, app, "no MCP servers configured");
    }
    let out = servers
        .iter()
        .map(|s| {
            let (icon, color) = if s.is_up() {
                (Icon::StateDone, t.ok)
            } else {
                (Icon::StateErr, t.error)
            };
            Line::from(vec![
                Span::styled(glyph(app, icon), Style::new().fg(color)),
                Span::styled(format!(" {} ", fit(&s.name, 18)), Style::new().fg(t.text)),
                dim(t, s.status.clone()),
            ])
        })
        .collect();
    lines(frame, area, out);
}

fn system(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let Some(s) = app.mission.snapshot.system.data.as_ref() else {
        return hint(
            frame,
            area,
            app,
            &feed_note(&app.mission.snapshot.system, "system readings"),
        );
    };
    let w = usize::from(area.width);
    let mut out = vec![Line::from(vec![
        label(t, "cpu "),
        Span::styled(format!("{:.0}%", s.cpu), Style::new().fg(t.text)),
        dim(t, format!("  {} cores", s.cores.len())),
    ])];
    // Cores in two columns, btop style.
    let cols = if w >= 36 { 2 } else { 1 };
    let col_w = (w / cols).max(10);
    let bar_w = col_w.saturating_sub(10).clamp(3, 16);
    let reserved = 3;
    let core_rows = usize::from(area.height).saturating_sub(reserved);
    for chunk in s.cores.chunks(cols).take(core_rows) {
        let mut spans = Vec::new();
        for (j, pct) in chunk.iter().enumerate() {
            let i = out.len() - 1;
            let idx = i * cols + j;
            spans.push(label(t, format!("{idx:>2} ")));
            spans.extend(bar_spans(pct / 100.0, bar_w, &t.gauge, t.dim));
            spans.push(dim(t, format!("{pct:>4.0}% ")));
        }
        out.push(Line::from(spans));
    }
    out.push(mem_line(t, "mem ", s.mem_used, s.mem_total, w));
    if s.swap_total > 0 {
        out.push(mem_line(t, "swap", s.swap_used, s.swap_total, w));
    }
    lines(frame, area, out);
}

fn mem_line(t: &Theme, name: &str, used: u64, total: u64, width: usize) -> Line<'static> {
    let gb = |b: u64| b as f64 / f64::from(1u32 << 30);
    let ratio = if total > 0 {
        used as f64 / total as f64
    } else {
        0.0
    };
    let text = format!(" {:.1}/{:.0} GB", gb(used), gb(total));
    let bar_w = width.saturating_sub(5 + text.chars().count()).clamp(3, 30);
    let mut spans = vec![label(t, format!("{name} "))];
    spans.extend(bar_spans(ratio, bar_w, &t.gauge, t.dim));
    spans.push(dim(t, text));
    Line::from(spans)
}

fn calendar(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let Some(u) = app.mission.usage() else {
        return hint(
            frame,
            area,
            app,
            "activity appears once transcripts are read",
        );
    };
    let days: Vec<_> = u.days.iter().rev().take(84).rev().collect();
    let counts: Vec<u32> = days
        .iter()
        .map(|d| d.spend.messages.min(u64::from(u32::MAX)) as u32)
        .collect();
    let weekday = days.last().map_or(0, |d| d.weekday as u8);
    let caption = format!(
        "{} messages in {} days",
        counts.iter().map(|c| u64::from(*c)).sum::<u64>(),
        counts.len()
    );
    let cap_h = u16::from(area.height >= 9);
    let map = Rect {
        height: area.height - cap_h,
        ..area
    };
    frame.render_widget(
        Heatmap::new(&counts, t.heat())
            .weekday_labels(true)
            .today_weekday(weekday)
            .label_color(t.muted),
        map,
    );
    if cap_h == 1 {
        frame.render_widget(
            Line::from(dim(t, fit(&caption, usize::from(area.width)))),
            row(area, area.height - 1),
        );
    }
}

/// Used by the focus peek: a todo's mark.
pub(super) fn todo_icon(status: TodoStatus) -> Icon {
    match status {
        TodoStatus::Completed => Icon::StateDone,
        TodoStatus::InProgress => Icon::StateRun,
        TodoStatus::Pending => Icon::StateIdle,
    }
}
