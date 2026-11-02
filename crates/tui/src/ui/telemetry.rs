//! The telemetry panels: usage limits, burn rate, token flow, context,
//! agent tree, todos, tool activity, files, git, CI, system, session wall,
//! activity calendar and environment. Each shows "waiting for data" until
//! its part of [`Telemetry`] is set.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::dashboard::{dim, fit, label, text};
use super::grid::chrome;
use super::grid::{Panel, PanelId, ViewModel};
use super::look::Icon;
use super::widgets::{
    GradientGauge, Heatmap, LineChart, MiniTable, Series, SparkStyle, Sparkline, Ticker, bar_spans,
};
use crate::telemetry::{Activity, CheckState, Gauge, Telemetry, TodoState};
use crate::theme::{Theme, mix};

/// A telemetry panel, by id.
pub struct Tele(PanelId);

/// The panel behind `id`; every id that isn't the dashboard's is one.
pub fn panel(id: PanelId) -> &'static dyn Panel {
    static PANELS: [Tele; 14] = [
        Tele(PanelId::Usage),
        Tele(PanelId::Burn),
        Tele(PanelId::Tokens),
        Tele(PanelId::Context),
        Tele(PanelId::Agents),
        Tele(PanelId::Todos),
        Tele(PanelId::Tools),
        Tele(PanelId::Files),
        Tele(PanelId::Git),
        Tele(PanelId::Ci),
        Tele(PanelId::System),
        Tele(PanelId::Wall),
        Tele(PanelId::Calendar),
        Tele(PanelId::Env),
    ];
    PANELS.iter().find(|p| p.0 == id).unwrap_or(&PANELS[0])
}

impl Tele {
    fn meta(&self) -> (&'static str, Icon, u8) {
        match self.0 {
            PanelId::Usage => ("usage limits", Icon::Usage, 6),
            PanelId::Burn => ("burn rate", Icon::Burn, 6),
            PanelId::Tokens => ("token flow", Icon::Tokens, 8),
            PanelId::Context => ("context", Icon::Context, 7),
            PanelId::Agents => ("agent tree", Icon::Agents, 11),
            PanelId::Todos => ("todo board", Icon::Todos, 9),
            PanelId::Tools => ("tool activity", Icon::Tools, 10),
            PanelId::Files => ("files touched", Icon::Files, 13),
            PanelId::Git => ("git", Icon::Git, 10),
            PanelId::Ci => ("ci / pr", Icon::Ci, 13),
            PanelId::System => ("system", Icon::System, 9),
            PanelId::Wall => ("session wall", Icon::Wall, 12),
            PanelId::Calendar => ("activity", Icon::Calendar, 14),
            _ => ("environment", Icon::Env, 15),
        }
    }

    fn accent(&self, t: &Theme) -> Color {
        match self.0 {
            PanelId::Usage | PanelId::Agents | PanelId::Wall => t.lead,
            PanelId::Burn | PanelId::Git => t.warn,
            PanelId::Tokens | PanelId::System => t.haiku,
            PanelId::Context => t.reviewer,
            PanelId::Todos | PanelId::Ci | PanelId::Calendar => t.ok,
            PanelId::Tools => t.builder,
            _ => t.label,
        }
    }

    /// Whether this panel's data has arrived.
    fn ready(&self, d: &Telemetry) -> bool {
        match self.0 {
            PanelId::Usage => d.usage.is_some(),
            PanelId::Burn => d.burn.is_some(),
            PanelId::Tokens => d.tokens.is_some(),
            PanelId::Context => d.context.is_some(),
            PanelId::Agents => d.agents.is_some(),
            PanelId::Todos => d.todos.is_some(),
            PanelId::Tools => d.tools.is_some(),
            PanelId::Files => d.files.is_some(),
            PanelId::Git => d.git.is_some(),
            PanelId::Ci => d.ci.is_some(),
            PanelId::System => d.system.is_some(),
            PanelId::Wall => d.wall.is_some(),
            PanelId::Calendar => d.calendar.is_some(),
            _ => d.env.is_some(),
        }
    }
}

impl Panel for Tele {
    fn id(&self) -> PanelId {
        self.0
    }
    fn title(&self) -> &'static str {
        self.meta().0
    }
    fn icon(&self) -> Icon {
        self.meta().1
    }
    fn min_size(&self) -> (u16, u16) {
        match self.0 {
            PanelId::Calendar => (18, 5),
            PanelId::Tokens | PanelId::Burn => (18, 4),
            _ => (16, 3),
        }
    }
    fn priority(&self) -> u8 {
        self.meta().2
    }
    fn render(&self, frame: &mut Frame, area: Rect, vm: &ViewModel, t: &Theme, tick: u64) {
        let ready = self.ready(vm.data);
        let accent = if ready { self.accent(t) } else { t.border };
        let chip = if ready { self.accent(t) } else { t.muted };
        let block = chrome::framed(
            self.0,
            vm.icon(self.icon()),
            vm,
            t,
            chip,
            vec![chrome::title(format!(" {} ", self.title()), chip)],
            None,
            chrome::Border::Rounded,
        )
        .border_style(Style::new().fg(if vm.focus == Some(self.0) {
            t.border_focus
        } else {
            accent
        }));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        if !ready {
            waiting(frame, inner, vm.icon(self.icon()), self.title(), t);
            return;
        }
        let d = vm.data;
        match self.0 {
            PanelId::Usage => {
                if let Some(u) = &d.usage {
                    gauges(frame, inner, vm, t, "usage", &u.gauges, u.note.as_deref());
                }
            }
            PanelId::Context => {
                if let Some(c) = &d.context {
                    gauges(frame, inner, vm, t, "context", &c.gauges, None);
                }
            }
            PanelId::Burn => burn(frame, inner, d, t),
            PanelId::Tokens => tokens(frame, inner, d, t),
            PanelId::Agents => agents(frame, inner, vm, d, t),
            PanelId::Todos => todos(frame, inner, d, t),
            PanelId::Tools => tools(frame, inner, d, t, tick),
            PanelId::Files => files(frame, inner, d, t),
            PanelId::Git => git(frame, inner, d, t),
            PanelId::Ci => ci(frame, inner, d, t),
            PanelId::System => system(frame, inner, vm, d, t),
            PanelId::Wall => wall(frame, inner, vm, d, t),
            PanelId::Calendar => calendar(frame, inner, d, t),
            _ => env(frame, inner, d, t),
        }
    }
    fn summary(&self, vm: &ViewModel, t: &Theme, _tick: u64) -> Line<'static> {
        let (title, icon, _) = self.meta();
        let head = Span::styled(
            format!(" {} {title} ", vm.icon(icon)),
            Style::new().fg(self.accent(t)),
        );
        let d = vm.data;
        let body = match self.0 {
            PanelId::Usage => d.usage.as_ref().map(|u| gauge_text(&u.gauges)),
            PanelId::Context => d.context.as_ref().map(|c| gauge_text(&c.gauges)),
            PanelId::Burn => d
                .burn
                .as_ref()
                .map(|b| format!("${:.2} today · ${:.2}/h", b.today_usd, b.per_hour_usd)),
            PanelId::Tokens => d.tokens.as_ref().map(|f| {
                f.series
                    .iter()
                    .map(|s| format!("{} {:.0}", s.name, s.points.last().copied().unwrap_or(0.0)))
                    .collect::<Vec<_>>()
                    .join(" · ")
            }),
            PanelId::Agents => d.agents.as_ref().map(|a| {
                let running = a
                    .nodes
                    .iter()
                    .filter(|n| n.state == Activity::Running)
                    .count();
                format!("{} agents · {running} running", a.nodes.len())
            }),
            PanelId::Todos => d.todos.as_ref().map(|td| {
                let done = td
                    .items
                    .iter()
                    .filter(|i| i.state == TodoState::Done)
                    .count();
                format!("{done}/{} done", td.items.len())
            }),
            PanelId::Tools => d.tools.as_ref().map(|tl| {
                let calls: f64 = tl.counts.iter().map(|c| c.1).sum();
                format!("{calls:.0} calls · {} tools", tl.counts.len())
            }),
            PanelId::Files => d.files.as_ref().map(|f| format!("{} files", f.files.len())),
            PanelId::Git => d
                .git
                .as_ref()
                .map(|g| format!("{} ↑{} ↓{} ~{}", g.branch, g.ahead, g.behind, g.modified)),
            PanelId::Ci => d.ci.as_ref().map(|c| {
                let failed = c
                    .checks
                    .iter()
                    .filter(|k| k.state == CheckState::Fail)
                    .count();
                format!("{} checks · {failed} failing", c.checks.len())
            }),
            PanelId::System => d
                .system
                .as_ref()
                .map(|s| format!("cpu {:.0}% · mem {:.0}%", s.cpu * 100.0, s.mem * 100.0)),
            PanelId::Wall => d
                .wall
                .as_ref()
                .map(|w| format!("{} sessions", w.sessions.len())),
            PanelId::Calendar => d
                .calendar
                .as_ref()
                .map(|c| format!("today {}", c.days.last().copied().unwrap_or(0))),
            _ => d.env.as_ref().map(|e| {
                e.pairs
                    .iter()
                    .map(|(k, v)| format!("{k} {v}"))
                    .collect::<Vec<_>>()
                    .join(" · ")
            }),
        };
        Line::from(vec![
            head,
            match body {
                Some(b) => text(t, b),
                None => dim(t, "waiting for data"),
            },
        ])
    }
}

/// The empty state: the panel's icon and title, and "waiting for data".
fn waiting(frame: &mut Frame, area: Rect, icon: &str, title: &str, t: &Theme) {
    let mut lines = vec![
        Line::from(Span::styled(
            format!("{icon}  {title}"),
            Style::new().fg(t.muted),
        )),
        Line::from(dim(t, "waiting for data")),
    ];
    if area.height < 2 {
        lines.remove(0);
    }
    let h = lines.len() as u16;
    let top = area.y + area.height.saturating_sub(h) / 2;
    let rect = Rect {
        y: top,
        height: h.min(area.height),
        ..area
    };
    frame.render_widget(Paragraph::new(lines).centered(), rect);
}

fn gauge_text(gauges: &[Gauge]) -> String {
    gauges
        .iter()
        .map(|g| format!("{} {:.0}%", g.label, g.ratio * 100.0))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// One gradient gauge per row (two rows each when there is room), and a note.
fn gauges(
    frame: &mut Frame,
    area: Rect,
    vm: &ViewModel,
    t: &Theme,
    key: &str,
    gauges: &[Gauge],
    note: Option<&str>,
) {
    let rows_each = if area.height as usize >= gauges.len() * 2 + usize::from(note.is_some()) {
        2
    } else {
        1
    };
    let mut y = area.y;
    for g in gauges {
        if y + rows_each > area.bottom() {
            break;
        }
        let ratio = vm.tween(&format!("{key}.{}", g.label), g.ratio);
        let detail = if g.detail.is_empty() {
            format!("{:.0}%", ratio * 100.0)
        } else {
            g.detail.clone()
        };
        frame.render_widget(
            GradientGauge::new(ratio, &t.gauge)
                .label(&g.label)
                .value(detail)
                .track(t.dim)
                .label_color(t.label),
            Rect::new(area.x, y, area.width, rows_each),
        );
        y += rows_each;
    }
    if let Some(note) = note
        && y < area.bottom()
    {
        frame.render_widget(
            Line::from(dim(t, fit(note, area.width as usize))),
            Rect::new(area.x, area.bottom() - 1, area.width, 1),
        );
    }
}

fn burn(frame: &mut Frame, area: Rect, d: &Telemetry, t: &Theme) {
    let Some(b) = &d.burn else { return };
    let [head, graph, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(u16::from(b.projection.is_some() && area.height >= 4)),
    ])
    .areas(area);
    frame.render_widget(
        Line::from(vec![
            Span::styled(
                format!("${:.2}", b.today_usd),
                Style::new().fg(t.warn).add_modifier(Modifier::BOLD),
            ),
            label(t, " today · "),
            text(t, format!("${:.2}/h", b.per_hour_usd)),
        ]),
        head,
    );
    frame.render_widget(
        Sparkline::new(&b.series, &t.gauge).style(SparkStyle::Block),
        graph,
    );
    if let Some(p) = &b.projection {
        frame.render_widget(Line::from(dim(t, fit(p, foot.width as usize))), foot);
    }
}

fn series_color(t: &Theme, i: usize, n: usize) -> Color {
    t.chart_at(if n <= 1 {
        1.0
    } else {
        i as f64 / (n - 1) as f64
    })
}

fn tokens(frame: &mut Frame, area: Rect, d: &Telemetry, t: &Theme) {
    let Some(f) = &d.tokens else { return };
    let n = f.series.len();
    let series: Vec<Series> = f
        .series
        .iter()
        .enumerate()
        .map(|(i, s)| Series {
            name: &s.name,
            points: &s.points,
            color: series_color(t, i, n),
        })
        .collect();
    frame.render_widget(
        LineChart::new(&series)
            .y_axis(true)
            .unit(&f.unit)
            .labels(t.muted)
            .axis(t.dim)
            .legend(true),
        area,
    );
}

fn agents(frame: &mut Frame, area: Rect, vm: &ViewModel, d: &Telemetry, t: &Theme) {
    let Some(a) = &d.agents else { return };
    let w = area.width as usize;
    let lines: Vec<Line> = a
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let last_sibling = a.nodes[i + 1..]
                .iter()
                .take_while(|m| m.depth >= n.depth)
                .all(|m| m.depth > n.depth);
            let indent = if n.depth == 0 {
                String::new()
            } else {
                format!(
                    "{}{}",
                    "  ".repeat(usize::from(n.depth) - 1),
                    if last_sibling { "└─ " } else { "├─ " }
                )
            };
            let color = t.model(n.model.as_deref());
            Line::from(vec![
                dim(t, indent),
                activity_mark(vm, n.state, t),
                Span::styled(
                    format!(" {}", fit(&n.name, w.saturating_sub(8))),
                    Style::new().fg(color),
                ),
                dim(
                    t,
                    n.model
                        .as_deref()
                        .map_or(String::new(), |m| format!(" {m}")),
                ),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn activity_mark(vm: &ViewModel, state: Activity, t: &Theme) -> Span<'static> {
    match state {
        Activity::Running => Span::styled(vm.spin().to_string(), Style::new().fg(t.lead)),
        Activity::Done => Span::styled("✓", Style::new().fg(t.ok)),
        Activity::Failed => Span::styled("✗", Style::new().fg(t.error)),
        Activity::Idle => Span::styled("·", Style::new().fg(t.muted)),
    }
}

fn todos(frame: &mut Frame, area: Rect, d: &Telemetry, t: &Theme) {
    let Some(td) = &d.todos else { return };
    let done = td
        .items
        .iter()
        .filter(|i| i.state == TodoState::Done)
        .count();
    let total = td.items.len().max(1);
    let w = area.width as usize;
    let mut head = vec![label(t, format!("{done}/{} ", td.items.len()))];
    head.extend(bar_spans(
        done as f64 / total as f64,
        w.saturating_sub(6),
        &[t.dim, t.ok],
        t.dim,
    ));
    let mut lines = vec![Line::from(head)];
    for item in &td.items {
        let (mark, color) = match item.state {
            TodoState::Done => ("[✓]", t.muted),
            TodoState::Active => ("[>]", t.lead),
            TodoState::Pending => ("[ ]", t.text),
        };
        let mut style = Style::new().fg(color);
        if item.state == TodoState::Done {
            style = style.add_modifier(Modifier::CROSSED_OUT);
        }
        lines.push(Line::from(vec![
            Span::styled(format!("{mark} "), Style::new().fg(color)),
            Span::styled(fit(&item.text, w.saturating_sub(4)), style),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn tools(frame: &mut Frame, area: Rect, d: &Telemetry, t: &Theme, tick: u64) {
    let Some(tl) = &d.tools else { return };
    let mut counts = tl.counts.clone();
    counts.sort_by(|a, b| b.1.total_cmp(&a.1));
    let peak = counts.first().map_or(1.0, |c| c.1.max(1.0));
    let w = area.width as usize;
    let ticker_h = u16::from(!tl.recent.is_empty() && area.height >= 3);
    let rows = area.height.saturating_sub(ticker_h) as usize;
    let lines: Vec<Line> = counts
        .iter()
        .take(rows)
        .map(|(name, n)| {
            let mut spans = vec![label(t, format!("{:<6} ", fit(name, 6)))];
            spans.extend(bar_spans(
                n / peak,
                w.saturating_sub(12),
                &[mix(t.dim, t.builder, 0.5), t.builder],
                t.dim,
            ));
            spans.push(text(t, format!(" {n:>3.0}")));
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
    if ticker_h == 1 {
        frame.render_widget(
            Ticker::new(&tl.recent)
                .tick(tick)
                .speed(2)
                .color(t.muted)
                .sep_color(t.dim),
            Rect::new(area.x, area.bottom() - 1, area.width, 1),
        );
    }
}

fn files(frame: &mut Frame, area: Rect, d: &Telemetry, t: &Theme) {
    let Some(f) = &d.files else { return };
    let rows = f
        .files
        .iter()
        .map(|(path, n)| {
            vec![
                Span::styled(path.clone(), Style::new().fg(t.text)),
                Span::styled(format!("{n}×"), Style::new().fg(t.label)),
            ]
        })
        .collect();
    frame.render_widget(
        MiniTable::new(rows, vec![0, 4])
            .right_align(vec![false, true])
            .header_color(t.muted),
        area,
    );
}

fn git(frame: &mut Frame, area: Rect, d: &Telemetry, t: &Theme) {
    let Some(g) = &d.git else { return };
    let w = area.width as usize;
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                fit(&g.branch, w.saturating_sub(12)),
                Style::new().fg(t.warn).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" ↑{}", g.ahead), Style::new().fg(t.ok)),
            Span::styled(format!(" ↓{}", g.behind), Style::new().fg(t.error)),
        ]),
        Line::from(vec![
            Span::styled(format!("+{} ", g.staged), Style::new().fg(t.ok)),
            Span::styled(format!("~{} ", g.modified), Style::new().fg(t.warn)),
            Span::styled(format!("?{}", g.untracked), Style::new().fg(t.muted)),
        ]),
    ];
    if let Some(c) = &g.last_commit {
        lines.push(Line::from(dim(t, fit(c, w))));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn ci(frame: &mut Frame, area: Rect, d: &Telemetry, t: &Theme) {
    let Some(c) = &d.ci else { return };
    let w = area.width as usize;
    let mut lines = Vec::new();
    if let Some(pr) = &c.pr {
        lines.push(Line::from(Span::styled(
            fit(pr, w),
            Style::new().fg(t.label),
        )));
    }
    for k in &c.checks {
        let (mark, color) = match k.state {
            CheckState::Pass => ("✓", t.ok),
            CheckState::Fail => ("✗", t.error),
            CheckState::Pending => ("●", t.warn),
            CheckState::Skipped => ("○", t.muted),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{mark} "), Style::new().fg(color)),
            text(t, fit(&k.name, w.saturating_sub(2))),
            dim(
                t,
                if k.detail.is_empty() {
                    String::new()
                } else {
                    format!(" {}", k.detail)
                },
            ),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn system(frame: &mut Frame, area: Rect, vm: &ViewModel, d: &Telemetry, t: &Theme) {
    let Some(s) = &d.system else { return };
    let cpu = vm.tween("system.cpu", s.cpu);
    let mem = vm.tween("system.mem", s.mem);
    let row = |y: u16| Rect::new(area.x, y, area.width, 1);
    frame.render_widget(
        GradientGauge::new(cpu, &t.gauge)
            .label("cpu")
            .value(format!("{:.0}%", cpu * 100.0))
            .track(t.dim)
            .label_color(t.label),
        row(area.y),
    );
    if area.height >= 2 {
        frame.render_widget(
            GradientGauge::new(mem, &t.gauge)
                .label("mem")
                .value(s.mem_detail.clone())
                .track(t.dim)
                .label_color(t.label),
            row(area.y + 1),
        );
    }
    if area.height >= 3 {
        let graph = Rect {
            y: area.y + 2,
            height: area.height - 2,
            ..area
        };
        let stops = [t.chart[0], t.chart[2]];
        frame.render_widget(
            Sparkline::new(&s.cpu_series, &stops)
                .style(SparkStyle::Braille)
                .max(1.0),
            graph,
        );
    }
}

fn wall(frame: &mut Frame, area: Rect, vm: &ViewModel, d: &Telemetry, t: &Theme) {
    let Some(wv) = &d.wall else { return };
    let w = area.width as usize;
    let name_w = w.saturating_sub(16).clamp(4, 16);
    let lines: Vec<Line> = wv
        .sessions
        .iter()
        .map(|s| {
            let color = t.model(Some(&s.model));
            let mut spans = vec![
                activity_mark(vm, s.state, t),
                Span::styled(
                    format!(" {:<name_w$} ", fit(&s.name, name_w)),
                    Style::new().fg(color),
                ),
            ];
            spans.extend(bar_spans(s.context, 5, &t.gauge, t.dim));
            spans.push(label(t, format!(" ${:.2}", s.cost_usd)));
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn calendar(frame: &mut Frame, area: Rect, d: &Telemetry, t: &Theme) {
    let Some(c) = &d.calendar else { return };
    let caption_h = u16::from(c.caption.is_some() && area.height >= 8);
    let map = Rect {
        height: area.height - caption_h,
        ..area
    };
    frame.render_widget(
        Heatmap::new(&c.days, t.heat())
            .weekday_labels(true)
            .today_weekday(c.today_weekday)
            .label_color(t.muted),
        map,
    );
    if let Some(caption) = c.caption.as_deref().filter(|_| caption_h == 1) {
        frame.render_widget(
            Line::from(dim(t, fit(caption, area.width as usize))),
            Rect::new(area.x, area.bottom() - 1, area.width, 1),
        );
    }
}

fn env(frame: &mut Frame, area: Rect, d: &Telemetry, t: &Theme) {
    let Some(e) = &d.env else { return };
    let key_w = e
        .pairs
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(4)
        .min(12) as u16;
    let rows = e
        .pairs
        .iter()
        .map(|(k, v)| {
            vec![
                Span::styled(k.clone(), Style::new().fg(t.label)),
                Span::styled(v.clone(), Style::new().fg(t.text)),
            ]
        })
        .collect();
    frame.render_widget(MiniTable::new(rows, vec![key_w, 0]), area);
}
