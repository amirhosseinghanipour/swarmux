//! The strip of sessions: one row each with what it runs, its context and
//! spend, how long, and the last line of its stream. Enter opens the job.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_application::console::ConsoleSnapshot;
use swarmux_domain::display_name;

use super::frame;
use crate::app::App;
use crate::deck::Focus;
use crate::mission::{Tile, TileState};
use crate::ui::look::IconSet;
use crate::ui::mission::age;
use crate::ui::widgets::bar_spans;
use crate::ui::work::util::{Lines, fit, span, usd};

fn state_word(s: TileState) -> &'static str {
    match s {
        TileState::Running => "running",
        TileState::Waiting => "waiting",
        TileState::Queued => "queued",
        TileState::Done => "done",
        TileState::Failed => "failed",
        TileState::Idle => "idle",
    }
}

/// The repository a session's work is in: the Deck's card of its job, else
/// the folder the session runs in.
fn repo_of(app: &App, snap: &ConsoleSnapshot, tile: &Tile) -> String {
    let from_card = tile.job.and_then(|j| {
        snap.deck
            .nodes
            .iter()
            .find(|n| n.job == Some(j.0))
            .and_then(|n| n.repo.clone())
    });
    from_card
        .or_else(|| tile.workspace.clone())
        .or_else(|| {
            app.sessions
                .iter()
                .find(|s| s.id == tile.session)
                .and_then(|s| s.cwd.file_name())
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "—".into())
}

/// The last thing the job's stream said: its log line without the time,
/// the actor and the kind in front.
fn last_line(app: &App, tile: &Tile) -> String {
    let from_stream = tile
        .job
        .and_then(|j| app.engine.jobs.progress(j))
        .and_then(|p| p.log.back().cloned())
        .map(|line| {
            line.split_whitespace()
                .skip(3)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|s| !s.is_empty());
    from_stream.unwrap_or_else(|| tile.task.clone())
}

pub(super) fn render(frame_: &mut Frame, area: Rect, app: &App, snap: &ConsoleSnapshot) {
    let t = &app.look.theme;
    let tiles = app.wall();
    let layout = app.deck_layout();
    let focused = app.deck_screen.focus == Focus::Sessions;
    let note = format!("{} attached · enter on a row opens its job", tiles.len());
    let block = frame(
        app,
        "SESSIONS",
        &note,
        if focused { t.border_focus } else { t.border },
    );
    let inner = block.inner(area);
    frame_.render_widget(block, area);
    if inner.width < 30 || inner.height == 0 {
        return;
    }
    if tiles.is_empty() {
        let new = crate::hints::key_or_name(app, crate::screens::sessions::NEW);
        frame_.render_widget(
            Paragraph::new(Line::from(span(
                format!("no sessions yet: press {new} on the sessions screen to create one"),
                Style::new().fg(t.muted),
            ))),
            inner,
        );
        return;
    }
    let w = usize::from(inner.width);
    // Columns drop from the least important when the strip is narrow.
    let show_repo = w >= 78;
    let show_age = w >= 60;
    let show_ctx = w >= 50;
    let name_w = 14;
    let repo_w = 16;
    let model_w = 10;
    let fixed = 2
        + name_w
        + 1
        + 9
        + if show_repo { repo_w + 1 } else { 0 }
        + model_w
        + 1
        + if show_ctx { 11 } else { 0 }
        + 8
        + if show_age { 6 } else { 0 };
    let last_w = w.saturating_sub(fixed + 1);

    let head = |s: &str, width: usize| span(format!("{s:<width$}"), Style::new().fg(t.dim));
    let mut lines: Lines = Vec::new();
    let mut header = vec![
        span("  ", Style::new()),
        head("SESSION", name_w + 1),
        head("STATE", 9),
    ];
    if show_repo {
        header.push(head("REPO", repo_w + 1));
    }
    header.push(head("MODEL", model_w + 1));
    if show_ctx {
        header.push(head("CONTEXT", 11));
    }
    header.push(head("SPEND", 8));
    if show_age {
        header.push(head("AGE", 6));
    }
    if last_w >= 8 {
        header.push(head("LAST FROM THE STREAM", last_w));
    }
    lines.push(Line::from(header));

    let rows = usize::from(inner.height)
        .saturating_sub(1)
        .min(layout.session_rows);
    let at = app.deck_screen.session_at.min(tiles.len() - 1);
    // Keep the selected row inside what is shown.
    let first = at.saturating_sub(rows.saturating_sub(1));
    let ascii = app.look.icons == IconSet::Ascii;
    for (i, tile) in tiles.iter().enumerate().skip(first).take(rows) {
        let selected = focused && i == at;
        let color = match tile.state {
            TileState::Running => t.text,
            TileState::Waiting => t.warn,
            TileState::Queued | TileState::Idle => t.muted,
            TileState::Done => t.ok,
            TileState::Failed => t.error,
        };
        let marker = if selected {
            span(
                if ascii { "> " } else { "▌ " },
                Style::new().fg(t.border_focus),
            )
        } else {
            span("  ", Style::new())
        };
        let mut spans: Vec<Span<'static>> = vec![
            marker,
            span(
                format!("{:<w$} ", fit(&tile.name, name_w), w = name_w),
                Style::new().fg(t.title).add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
            span(
                format!("{:<9}", state_word(tile.state)),
                Style::new().fg(color),
            ),
        ];
        if show_repo {
            spans.push(span(
                format!(
                    "{:<w$} ",
                    fit(&repo_of(app, snap, tile), repo_w),
                    w = repo_w
                ),
                Style::new().fg(t.muted),
            ));
        }
        spans.push(span(
            format!(
                "{:<w$} ",
                fit(&display_name(&tile.model), model_w),
                w = model_w
            ),
            Style::new().fg(t.model(Some(&tile.model))),
        ));
        if show_ctx {
            match tile.context {
                Some(c) => {
                    spans.extend(bar_spans(c.clamp(0.0, 1.0), 5, &t.gauge, t.dim));
                    spans.push(span(
                        format!(" {:>3.0}% ", c * 100.0),
                        Style::new().fg(t.text),
                    ));
                }
                None => spans.push(span(format!("{:<11}", "—"), Style::new().fg(t.dim))),
            }
        }
        spans.push(match tile.cost {
            Some(c) => span(format!("{:<8}", usd(c)), Style::new().fg(t.text)),
            None => span(format!("{:<8}", "—"), Style::new().fg(t.dim)),
        });
        if show_age {
            spans.push(span(
                format!(
                    "{:<6}",
                    tile.elapsed_ms.map_or_else(|| "—".to_string(), age)
                ),
                Style::new().fg(t.muted),
            ));
        }
        if last_w >= 8 {
            spans.push(span(
                fit(&last_line(app, tile), last_w),
                Style::new().fg(t.muted),
            ));
        }
        lines.push(Line::from(spans));
    }
    if tiles.len() > rows + first
        && let Some(last) = lines.last_mut()
    {
        // More sessions than the strip lists.
        let hidden = tiles.len() - first - rows;
        last.spans
            .push(span(format!("  +{hidden} more"), Style::new().fg(t.dim)));
    }
    frame_.render_widget(Paragraph::new(lines), inner);
}
