//! The session wall: one tile per session in a fixed order, drawn as small
//! multiples with identical scales. Narrow windows get a list.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph},
};
use swarmux_domain::display_name;

use super::layout::WallMode;
use super::{age, glyph, panel, sigil};
use crate::app::App;
use crate::mission::{Tile, TileState, Tone};
use crate::theme::Theme;
use crate::ui::dashboard::{dim, fit, label};
use crate::ui::look::Icon;
use crate::ui::widgets::{SparkStyle, Sparkline, bar_spans};

/// Narrowest a card gets (borders included).
const CARD_MIN_WIDTH: u16 = 30;

pub fn render(frame: &mut Frame, area: Rect, app: &App, tiles: &[Tile], mode: WallMode) {
    let t = &app.look.theme;
    let meta = Line::from(dim(
        t,
        format!(
            " {} session{} ",
            tiles.len(),
            if tiles.len() == 1 { "" } else { "s" }
        ),
    ));
    let block = panel(t, glyph(app, Icon::Wall), "wall", meta, t.border);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 12 || inner.height == 0 {
        return;
    }
    if tiles.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(dim(
                t,
                fit(
                    "no sessions yet · press 6, then n, to create one",
                    usize::from(inner.width),
                ),
            ))),
            Rect { height: 1, ..inner },
        );
        return;
    }
    if let WallMode::Cards { height, cap, slots } = mode {
        let cols = usize::from((inner.width / CARD_MIN_WIDTH).max(1));
        let rows = usize::from(inner.height / height);
        let room = (cols * rows).min(cap);
        if tiles.len() <= room {
            let shown = if slots { room } else { tiles.len() };
            return render_cards(frame, inner, app, tiles, (cols, height, shown));
        }
    }
    render_list(frame, inner, app, tiles);
}

fn state_style(t: &Theme, s: TileState) -> (Color, &'static str) {
    match s {
        TileState::Running => (t.ok, "running"),
        TileState::Waiting => (t.warn, "waiting"),
        TileState::Queued => (t.muted, "queued"),
        TileState::Done => (t.muted, "done"),
        TileState::Failed => (t.error, "failed"),
        TileState::Idle => (t.dim, "idle"),
    }
}

fn state_glyph(app: &App, tile: &Tile) -> Span<'static> {
    let t = &app.look.theme;
    let (color, _) = state_style(t, tile.state);
    let icon = match tile.state {
        TileState::Running => Icon::StateRun,
        TileState::Waiting | TileState::Queued => Icon::StateWait,
        TileState::Done => Icon::StateDone,
        TileState::Failed => Icon::StateErr,
        TileState::Idle => Icon::StateIdle,
    };
    Span::styled(glyph(app, icon), Style::new().fg(color))
}

fn sigil_span(t: &Theme, tile: &Tile) -> Span<'static> {
    Span::styled(
        sigil(&tile.name).to_string(),
        Style::new()
            .fg(t.model(Some(&tile.model)))
            .add_modifier(Modifier::BOLD),
    )
}

/// `Sonnet 4.5 · lean · acceptEdits`
fn badge(tile: &Tile) -> String {
    let mut parts = vec![display_name(&tile.model)];
    if tile.lean {
        parts.push("lean".into());
    }
    if !tile.permission_mode.is_empty() {
        parts.push(tile.permission_mode.clone());
    }
    parts.join(" · ")
}

fn money(usd: Option<f64>) -> String {
    usd.map_or("—".to_string(), |usd| format!("${usd:.2}"))
}

/// The busiest minute of any tile: every sparkline shares it.
fn shared_scale(tiles: &[Tile]) -> f64 {
    tiles
        .iter()
        .flat_map(|t| t.flow.iter().copied())
        .fold(1.0, f64::max)
}

/// Cards in stable slots: slot `i` is always row `i / cols`, column `i % cols`.
/// Slots past the last session are drawn faintly so the grid does not shift.
fn render_cards(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    tiles: &[Tile],
    (cols, height, shown): (usize, u16, usize),
) {
    let t = &app.look.theme;
    let scale = shared_scale(tiles);
    let rows = shown.div_ceil(cols);
    let row_areas = Layout::vertical(vec![Constraint::Length(height); rows]).split(area);
    for (r, row_area) in row_areas.iter().enumerate() {
        let col_areas =
            Layout::horizontal(vec![Constraint::Ratio(1, cols as u32); cols]).split(*row_area);
        for (c, cell) in col_areas.iter().enumerate() {
            let slot = r * cols + c;
            if slot >= shown {
                break;
            }
            match tiles.get(slot) {
                Some(tile) => render_card(frame, *cell, app, tile, scale, t),
                None => render_free(frame, *cell, t, slot + 1),
            }
        }
    }
}

fn render_free(frame: &mut Frame, area: Rect, t: &Theme, number: usize) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t.dim))
        .title(Line::from(dim(t, format!(" slot {number} · free "))));
    frame.render_widget(block, area);
}

fn render_card(frame: &mut Frame, area: Rect, app: &App, tile: &Tile, scale: f64, t: &Theme) {
    let (color, word) = state_style(t, tile.state);
    let title = Line::from(vec![
        Span::raw(" "),
        sigil_span(t, tile),
        Span::styled(
            format!(" {} ", fit(&tile.name, 18)),
            Style::new().fg(t.title),
        ),
    ]);
    let meta = Line::from(vec![
        Span::raw(" "),
        state_glyph(app, tile),
        Span::styled(format!(" {word} "), Style::new().fg(color)),
    ]);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t.border))
        .style(Style::new().bg(t.panel_bg))
        .title(title)
        .title(meta.right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 4 || inner.width < 10 {
        return;
    }
    let w = usize::from(inner.width);
    let line = |n: u16| Rect {
        y: inner.y + n,
        height: 1,
        ..inner
    };
    let tall = inner.height >= 6;

    frame.render_widget(
        Line::from(Span::styled(fit(&badge(tile), w), Style::new().fg(t.label))),
        line(0),
    );
    let task = if tile.task.is_empty() {
        "nothing sent yet".to_string()
    } else {
        tile.task.clone()
    };
    frame.render_widget(
        Line::from(vec![
            dim(t, "› "),
            Span::styled(fit(&task, w.saturating_sub(2)), Style::new().fg(t.text)),
        ]),
        line(1),
    );

    // Context meter, then the activity sparkline in what is left.
    let bar_w = (w / 3).clamp(6, 14);
    let ctx = tile.context.unwrap_or(0.0);
    let mut spans = vec![label(t, "ctx ")];
    spans.extend(bar_spans(ctx, bar_w, &t.gauge, t.dim));
    spans.push(Span::styled(
        match tile.context {
            Some(c) => format!(" {:>3.0}% ", c * 100.0),
            None => "   — ".to_string(),
        },
        Style::new().fg(t.muted),
    ));
    let used = Line::from(spans.clone()).width() as u16;
    frame.render_widget(Line::from(spans), line(2));
    if inner.width > used + 3 {
        frame.render_widget(
            Sparkline::new(&tile.flow, &t.chart)
                .style(SparkStyle::Block)
                .max(scale),
            Rect::new(inner.x + used, inner.y + 2, inner.width - used, 1),
        );
    }

    let mut next = 3;
    if tall {
        // Todo progress, then done-gate, review and guard chips.
        let todo = match tile.todos {
            Some((done, all)) => {
                let mut spans = vec![label(t, "todo ")];
                spans.extend(bar_spans(
                    f64::from(done) / f64::from(all.max(1)),
                    bar_w,
                    &t.gauge,
                    t.dim,
                ));
                spans.push(Span::styled(
                    format!(" {done}/{all}"),
                    Style::new().fg(t.muted),
                ));
                Line::from(spans)
            }
            None => Line::from(dim(t, "todo  no list")),
        };
        frame.render_widget(todo, line(next));
        next += 1;
        let chips: Vec<Span> = if tile.chips.is_empty() {
            vec![dim(t, "gate runs when the agent stops")]
        } else {
            tile.chips
                .iter()
                .flat_map(|c| {
                    let (icon, color) = match c.tone {
                        Tone::Good => (Icon::Result, t.ok),
                        Tone::Warn => (Icon::Concerns, t.warn),
                        Tone::Bad => (Icon::Failed, t.error),
                        Tone::Info => (Icon::StateRun, t.muted),
                    };
                    [
                        Span::styled(glyph(app, icon), Style::new().fg(color)),
                        Span::styled(format!(" {}  ", c.text), Style::new().fg(t.muted)),
                    ]
                })
                .collect()
        };
        frame.render_widget(Line::from(chips), line(next));
        next += 1;
    }
    let elapsed = tile.elapsed_ms.map_or("—".to_string(), age);
    frame.render_widget(
        Line::from(vec![
            Span::styled(elapsed, Style::new().fg(t.muted)),
            dim(t, " · "),
            Span::styled(money(tile.cost), Style::new().fg(t.muted)),
        ]),
        line(next),
    );
}

/// One row per session: below the tile breakpoint or with many sessions.
fn render_list(frame: &mut Frame, area: Rect, app: &App, tiles: &[Tile]) {
    let t = &app.look.theme;
    let w = usize::from(area.width);
    let room = usize::from(area.height);
    let shown = if tiles.len() > room {
        room.saturating_sub(1)
    } else {
        tiles.len()
    };
    let mut lines: Vec<Line> = tiles
        .iter()
        .take(shown)
        .map(|tile| list_row(app, tile, w))
        .collect();
    if shown < tiles.len() {
        lines.push(Line::from(dim(
            t,
            format!("+{} more sessions", tiles.len() - shown),
        )));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn list_row(app: &App, tile: &Tile, width: usize) -> Line<'static> {
    let t = &app.look.theme;
    let ctx = tile
        .context
        .map_or("   —".to_string(), |c| format!("{:>3.0}%", c * 100.0));
    let tail = format!(
        " {ctx} {:>6} {:>5}",
        money(tile.cost),
        tile.elapsed_ms.map_or("—".to_string(), age)
    );
    let name_w = 14.min(width / 4);
    let badge_w = if width >= 90 { 22 } else { 0 };
    let head = vec![
        sigil_span(t, tile),
        Span::raw(" "),
        state_glyph(app, tile),
        Span::styled(
            format!(" {:<name_w$} ", fit(&tile.name, name_w)),
            Style::new().fg(t.title),
        ),
    ];
    let mut spans = head;
    if badge_w > 0 {
        spans.push(Span::styled(
            format!("{:<badge_w$} ", fit(&badge(tile), badge_w)),
            Style::new().fg(t.label),
        ));
    }
    let used = Line::from(spans.clone()).width() + tail.chars().count();
    let task_w = width.saturating_sub(used + 1);
    let task = fit(&tile.task, task_w);
    let pad = task_w.saturating_sub(task.chars().count());
    spans.push(Span::styled(task, Style::new().fg(t.text)));
    spans.push(Span::raw(" ".repeat(pad + 1)));
    spans.push(Span::styled(tail, Style::new().fg(t.muted)));
    Line::from(spans)
}
