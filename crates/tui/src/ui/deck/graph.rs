//! The task graph: cards in columns from producer to consumer, the lines
//! between them, a legend, and the selected card's relations in words. At
//! a width the columns do not fit it is a list.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph},
};
use swarmux_application::console::{
    ConsoleSnapshot, DeckNode, DepState, EdgeKind, Lifecycle, NodeState, OutcomeView,
};
use swarmux_domain::display_name;

use super::frame;
use crate::app::App;
use crate::deck::layout::{self, Geometry, Metrics};
use crate::deck::{Focus, Plan, is_held, label, phrase, relations};
use crate::theme::Theme;
use crate::ui::look::IconSet;
use crate::ui::work::util::{Lines, fit, heading, span, usd};

/// How a card's state looks: a shape, a word and a colour. The shape says
/// it without colour.
pub(super) struct StateLook {
    pub shape: &'static str,
    pub word: &'static str,
    pub color: Color,
}

pub(super) fn outcome_of<'a>(snap: &'a ConsoleSnapshot, id: &str) -> Option<&'a OutcomeView> {
    snap.groups
        .iter()
        .flat_map(|g| &g.outcomes)
        .find(|o| o.id == id)
}

/// The shape, word and colour of a card: hollow square while the work is
/// not yet proven, solid once verified, pause bars while a call is held,
/// a diagonal-cut square when it is blocked or escalated.
pub(super) fn state_look(app: &App, snap: &ConsoleSnapshot, node: &DeckNode) -> StateLook {
    let t = &app.look.theme;
    let ascii = app.look.icons == IconSet::Ascii;
    let pick = |fancy: &'static str, plain: &'static str| if ascii { plain } else { fancy };
    let (open, solid, held, cut) = (
        pick("□ ", "o "),
        pick("■ ", "# "),
        pick("▌▐", "||"),
        pick("▚ ", "X "),
    );
    if is_held(snap, node) {
        return StateLook {
            shape: held,
            word: "HELD",
            color: t.warn,
        };
    }
    let verified = node.checks.total > 0 && node.checks.passed >= node.checks.total;
    match node.state {
        NodeState::Waiting => StateLook {
            shape: open,
            word: "WAITING",
            color: t.muted,
        },
        NodeState::Queued => StateLook {
            shape: open,
            word: "QUEUED",
            color: t.muted,
        },
        NodeState::Running => StateLook {
            shape: open,
            word: "RUNNING",
            color: t.text,
        },
        NodeState::Verifying => StateLook {
            shape: open,
            word: "VERIFYING",
            color: t.text,
        },
        NodeState::Ready => StateLook {
            shape: solid,
            word: "READY",
            color: t.ok,
        },
        NodeState::Done => StateLook {
            shape: solid,
            word: if verified { "VERIFIED" } else { "DONE" },
            color: t.ok,
        },
        NodeState::Failed => {
            let escalated = outcome_of(snap, &node.outcome)
                .is_some_and(|o| o.lifecycle == Lifecycle::Escalated);
            StateLook {
                shape: cut,
                word: if escalated { "ESCALATED" } else { "FAILED" },
                color: t.error,
            }
        }
        NodeState::Stopped => StateLook {
            shape: cut,
            word: "STOPPED",
            color: t.muted,
        },
    }
}

/// One dot per configured check: passed `■`, failed `×`, not yet `·`, and
/// how many of how many. A card with no checks says so.
pub(super) fn dots(app: &App, node: &DeckNode, room: usize) -> Vec<Span<'static>> {
    let t = &app.look.theme;
    let c = node.checks;
    if c.total == 0 {
        return vec![span("no checks configured", Style::new().fg(t.dim))];
    }
    let ascii = app.look.icons == IconSet::Ascii;
    let count = format!(" {}/{}", c.passed, c.total);
    let room = room.saturating_sub(count.chars().count());
    let total = c.total as usize;
    // Spaced when they fit so, packed when not, cut with `…` when even
    // packed they do not.
    let step = if total * 2 <= room { 2 } else { 1 };
    let shown = (room / step).min(total);
    let mut spans = Vec::new();
    for i in 0..shown {
        let (ch, color) = if i < c.passed as usize {
            (if ascii { "#" } else { "■" }, t.ok)
        } else if i < (c.passed + c.failed) as usize {
            (if ascii { "x" } else { "×" }, t.error)
        } else {
            (if ascii { "." } else { "·" }, t.dim)
        };
        let text = if step == 2 {
            format!("{ch} ")
        } else {
            ch.to_string()
        };
        spans.push(span(text, Style::new().fg(color)));
    }
    if shown < total {
        spans.push(span("…", Style::new().fg(t.dim)));
    }
    spans.push(span(count, Style::new().fg(t.muted)));
    spans
}

/// What a card's frame names: its repository, else the mission it belongs
/// to, else that it has none.
fn context_of(node: &DeckNode) -> String {
    match (&node.repo, node.outcome.strip_prefix("mission:")) {
        (Some(repo), _) => repo.clone(),
        (None, Some(slug)) => slug.to_string(),
        (None, None) => "no repository".to_string(),
    }
}

/// The model a card's job runs on, from the facts of the run.
fn model_of(app: &App, node: &DeckNode) -> Option<String> {
    let job = node.job?;
    app.mission
        .facts
        .iter()
        .find(|f| f.job_id.0 == job)
        .map(|f| display_name(&f.model))
}

fn edge_style(t: &Theme, look: (EdgeKind, DepState, bool)) -> Style {
    let color = match look.1 {
        DepState::Met => t.ok,
        DepState::Pending => t.label,
        DepState::Blocked => t.error,
    };
    let style = Style::new().fg(color);
    if look.2 {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

fn plain_glyph(c: char) -> char {
    match c {
        '─' | '┈' | '╌' => '-',
        '│' | '┊' | '╎' => '|',
        '▶' => '>',
        ' ' => ' ',
        _ => '+',
    }
}

pub(super) fn render(frame_: &mut Frame, area: Rect, app: &App, snap: &ConsoleSnapshot) {
    let t = &app.look.theme;
    let plan = Plan::of(snap);
    let sel = app.deck_screen.current(&plan);
    let focused = app.deck_screen.focus == Focus::Graph;
    let width = usize::from(area.width.saturating_sub(2));

    // Which form, and the note at the frame's right.
    let reserve = |h: u16| match h {
        18.. => 3,
        12..=17 => 2,
        _ => 1,
    };
    let inner_h = area.height.saturating_sub(2);
    let reserved = reserve(inner_h).min(inner_h);
    let view = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: inner_h - reserved,
    };
    let metrics = layout::fit(plan.layered.columns.len().max(1), view.width)
        .filter(|_| view.height >= layout::CARD_H);
    let note = match (&metrics, plan.is_empty()) {
        (_, true) => String::new(),
        (Some(_), _) => "producer → consumer · every line is a recorded relationship".into(),
        (None, _) => "as a list at this size".into(),
    };
    let color = if focused { t.border_focus } else { t.border };
    let block = frame(app, "TASK GRAPH", &note, color);
    frame_.render_widget(block, area);
    if view.width < 20 {
        return;
    }

    if plan.is_empty() {
        empty(frame_, view, app, width);
        return;
    }

    let mut hidden = 0usize;
    match metrics {
        Some(m) => hidden = render_graph(frame_, view, app, snap, &plan, sel, m),
        None => render_list(frame_, view, app, snap, &plan, sel),
    }
    if hidden > 0 {
        // Said at the frame's bottom edge, where nothing else is.
        let text = format!(" {hidden} more out of view · scroll with j/k ");
        let w = u16::try_from(text.chars().count()).unwrap_or(0);
        if area.width > w + 4 {
            frame_.render_widget(
                Paragraph::new(Line::from(span(text, Style::new().fg(t.muted)))),
                Rect {
                    x: area.x + area.width - w - 2,
                    y: area.y + area.height - 1,
                    width: w,
                    height: 1,
                },
            );
        }
    }

    // The selected card in words, then the legend.
    let base = view.y + view.height;
    let foot = Rect {
        x: view.x,
        y: base,
        width: view.width,
        height: reserved,
    };
    let mut lines: Lines = Vec::new();
    lines.push(detail(app, snap, &plan, sel, width));
    if reserved >= 2 {
        lines.push(legend_edges(app, width));
    }
    if reserved >= 3 {
        lines.push(legend_shapes(app, width));
    }
    frame_.render_widget(Paragraph::new(lines), foot);
}

fn empty(frame: &mut Frame, area: Rect, app: &App, width: usize) {
    let t = &app.look.theme;
    let new = crate::hints::key_or_name(app, crate::keys::PROMPT_SEND);
    let lines: Lines = vec![
        Line::from(span("no jobs yet", Style::new().fg(t.text))),
        Line::from(span(
            fit(
                &format!(
                    "a card appears here for every job and every task of a plan; press {new} on a session to start one"
                ),
                width,
            ),
            Style::new().fg(t.muted),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

/// Draws the columns. Returns how many cards are not fully in view.
fn render_graph(
    frame: &mut Frame,
    view: Rect,
    app: &App,
    snap: &ConsoleSnapshot,
    plan: &Plan,
    sel: Option<usize>,
    m: Metrics,
) -> usize {
    let t = &app.look.theme;
    let geo: Geometry = layout::geometry(&plan.input, &plan.layered, m, view.width, sel);
    let max = geo.height.saturating_sub(view.height);
    let mut scroll = app.deck_screen.scroll.get().min(max);
    if let Some((top, bottom)) = sel.and_then(|s| geo.rows_of(s)) {
        scroll = layout::follow(scroll, top, bottom, view.height).min(max);
    }
    app.deck_screen.scroll.set(scroll);

    let ascii = app.look.icons == IconSet::Ascii;
    let buf = frame.buffer_mut();
    for y in 0..view.height {
        for x in 0..view.width.min(geo.canvas.width) {
            let cell = geo.canvas.cell(x, y + scroll);
            let g = cell.glyph();
            if g == ' ' {
                continue;
            }
            let Some(look) = cell.look() else { continue };
            let c = if ascii { plain_glyph(g) } else { g };
            let target = &mut buf[(view.x + x, view.y + y)];
            target.set_char(c);
            target.set_style(edge_style(t, look));
        }
    }
    if let Some(top) = geo.shelf_top
        && top >= scroll
        && top < scroll + view.height
    {
        let line = heading(
            app,
            "NO RELATIONSHIPS",
            "",
            usize::from(view.width.saturating_sub(geo.shelf_x)),
            t.muted,
        );
        frame.render_widget(
            Paragraph::new(line),
            Rect {
                x: view.x + geo.shelf_x,
                y: view.y + top - scroll,
                width: view.width.saturating_sub(geo.shelf_x),
                height: 1,
            },
        );
    }
    let mut hidden = 0;
    for (i, node) in snap.deck.nodes.iter().enumerate() {
        let Some(p) = geo.cards.get(i).copied().flatten() else {
            continue;
        };
        if p.y < scroll || p.y + m.card_h > scroll + view.height {
            hidden += 1;
            continue;
        }
        let rect = Rect {
            x: view.x + p.x,
            y: view.y + p.y - scroll,
            width: m.card_w.min(view.width.saturating_sub(p.x)),
            height: m.card_h,
        };
        card(frame, rect, app, snap, node, sel == Some(i));
    }
    if scroll > 0 {
        mark(frame, view, "▲", 0, t);
    }
    if scroll < max {
        mark(frame, view, "▼", view.height - 1, t);
    }
    hidden
}

fn mark(frame: &mut Frame, view: Rect, text: &str, row: u16, t: &Theme) {
    frame.render_widget(
        Paragraph::new(Line::from(span(text.to_string(), Style::new().fg(t.muted)))),
        Rect {
            x: view.x + view.width - 1,
            y: view.y + row,
            width: 1,
            height: 1,
        },
    );
}

/// One card:
///
/// ```text
/// ┌ api-gateway ─────── opus 5.5 ┐
/// │ □ #7 VERIFYING        $9.14  │
/// │   ■ ■ · ■ ■ · · ·  3/9       │
/// │   Invoice totals round half… │
/// └──────────────────────────────┘
/// ```
fn card(
    frame: &mut Frame,
    rect: Rect,
    app: &App,
    snap: &ConsoleSnapshot,
    node: &DeckNode,
    selected: bool,
) {
    let t = &app.look.theme;
    let look = state_look(app, snap, node);
    let iw = usize::from(rect.width.saturating_sub(2));
    let border = if selected {
        t.border_focus
    } else if node.state == NodeState::Failed {
        t.error
    } else if look.word == "HELD" {
        t.warn
    } else {
        t.border
    };
    let mut block = Block::bordered()
        .border_type(if selected {
            BorderType::Double
        } else {
            BorderType::Plain
        })
        .border_style(Style::new().fg(border))
        .style(Style::new().bg(t.panel_paint()));
    let repo = context_of(node);
    block = block.title(Line::from(span(
        format!(" {} ", fit(&repo, iw.saturating_sub(14).max(6))),
        Style::new()
            .fg(if selected { t.title } else { t.text })
            .add_modifier(Modifier::BOLD),
    )));
    if let Some(model) = model_of(app, node) {
        block = block.title(
            Line::from(span(
                format!(" {} ", fit(&model, 12)),
                Style::new().fg(t.model(Some(&model))),
            ))
            .right_aligned(),
        );
    }
    let strong = Style::new().add_modifier(Modifier::BOLD);
    let name = label(node);
    let spend = usd(node.spend_usd);
    let head_used = 1 + 2 + 1 + name.chars().count() + 1 + look.word.chars().count();
    let pad = iw.saturating_sub(head_used + spend.chars().count() + 1);
    let line1 = Line::from(vec![
        span(" ", Style::new()),
        span(look.shape, Style::new().fg(look.color)),
        span(" ", Style::new()),
        span(name, Style::new().fg(t.label)),
        span(" ", Style::new()),
        span(look.word, strong.fg(look.color)),
        span(" ".repeat(pad), Style::new()),
        span(spend, Style::new().fg(t.muted)),
    ]);
    let mut line2 = vec![span("   ", Style::new())];
    line2.extend(dots(app, node, iw.saturating_sub(4)));
    let line3 = Line::from(span(
        format!("   {}", fit(&node.title, iw.saturating_sub(4))),
        Style::new().fg(if selected { t.title } else { t.text }),
    ));
    frame.render_widget(
        Paragraph::new(vec![line1, Line::from(line2), line3]).block(block),
        rect,
    );
}

/// The list form: two rows a card, in the order `j` and `k` walk.
fn render_list(
    frame: &mut Frame,
    view: Rect,
    app: &App,
    snap: &ConsoleSnapshot,
    plan: &Plan,
    sel: Option<usize>,
) {
    let t = &app.look.theme;
    let w = usize::from(view.width);
    let at = sel.and_then(|s| plan.order.iter().position(|n| *n == s));
    let (top, bottom) = at.map_or((0, 2), |p| {
        (
            u16::try_from(p * 2).unwrap_or(0),
            u16::try_from(p * 2 + 2).unwrap_or(2),
        )
    });
    let total = u16::try_from(plan.order.len() * 2).unwrap_or(u16::MAX);
    let max = total.saturating_sub(view.height);
    let scroll = layout::follow(
        u16::try_from(app.deck_screen.list_scroll.get()).unwrap_or(0),
        top,
        bottom,
        view.height,
    )
    .min(max);
    app.deck_screen.list_scroll.set(usize::from(scroll));

    let mut lines: Lines = Vec::new();
    for node_ix in &plan.order {
        let node = &snap.deck.nodes[*node_ix];
        let selected = sel == Some(*node_ix);
        let look = state_look(app, snap, node);
        let marker = if selected {
            span("▌", Style::new().fg(t.border_focus))
        } else {
            span(" ", Style::new())
        };
        let name = format!("{:<5}", fit(&label(node), 5));
        let repo = node.repo.as_deref().unwrap_or("—");
        let mut l1 = vec![
            marker.clone(),
            span(look.shape, Style::new().fg(look.color)),
            span(" ", Style::new()),
            span(name, Style::new().fg(t.label)),
            span(
                format!("{:<10}", look.word),
                Style::new().fg(look.color).add_modifier(Modifier::BOLD),
            ),
            span(
                format!("{:<14} ", fit(repo, 13)),
                Style::new().fg(if selected { t.title } else { t.text }),
            ),
        ];
        let used: usize = 1 + 2 + 1 + 5 + 10 + 15;
        l1.extend(dots(app, node, w.saturating_sub(used + 2).min(24)));
        lines.push(Line::from(l1));

        let rels: Vec<String> = relations(snap, plan, *node_ix)
            .iter()
            .map(|r| phrase(snap, r))
            .collect();
        let tail = if rels.is_empty() {
            "no dependencies".to_string()
        } else {
            rels.join(" · ")
        };
        let text = format!("{} · {tail}", node.title);
        lines.push(Line::from(vec![
            marker,
            span("   ", Style::new()),
            span(
                fit(&text, w.saturating_sub(5)),
                Style::new().fg(if selected { t.text } else { t.muted }),
            ),
        ]));
    }
    let shown: Lines = lines
        .into_iter()
        .skip(usize::from(scroll))
        .take(usize::from(view.height))
        .collect();
    frame.render_widget(Paragraph::new(shown), view);
    if scroll > 0 {
        mark(frame, view, "▲", 0, t);
    }
    if scroll < max {
        mark(frame, view, "▼", view.height - 1, t);
    }
}

/// The selected card in words: who it is, then every relation it has,
/// the ones the picture leaves out marked.
fn detail(
    app: &App,
    snap: &ConsoleSnapshot,
    plan: &Plan,
    sel: Option<usize>,
    width: usize,
) -> Line<'static> {
    let t = &app.look.theme;
    let Some(i) = sel else {
        return Line::default();
    };
    let node = &snap.deck.nodes[i];
    let rels = relations(snap, plan, i);
    let head = format!("{} {}", label(node), node.title);
    let mut spans = vec![span(
        format!("{} ", fit(&head, width / 2)),
        Style::new().fg(t.title).add_modifier(Modifier::BOLD),
    )];
    let used = spans[0].content.chars().count();
    if rels.is_empty() {
        spans.push(span("no relations", Style::new().fg(t.muted)));
    } else {
        let text = rels
            .iter()
            .map(|r| match r.note {
                Some(n) => format!("{} ({n})", phrase(snap, r)),
                None => phrase(snap, r),
            })
            .collect::<Vec<_>>()
            .join(" · ");
        spans.push(span(
            fit(&text, width.saturating_sub(used)),
            Style::new().fg(t.muted),
        ));
    }
    Line::from(spans)
}

fn legend_edges(app: &App, width: usize) -> Line<'static> {
    let t = &app.look.theme;
    let ascii = app.look.icons == IconSet::Ascii;
    let (solid, contract, claim) = if ascii {
        ("--", "--", "--")
    } else {
        ("──", "┈┈", "╌╌")
    };
    let mut spans = Vec::new();
    for (glyph, color, text) in [
        (solid, t.ok, "task dependency, met"),
        (solid, t.label, "pending"),
        (solid, t.error, "blocked"),
    ] {
        spans.push(span(format!("{glyph} "), Style::new().fg(color)));
        spans.push(span(format!("{text}  "), Style::new().fg(t.muted)));
    }
    spans.push(span(format!("{contract} "), Style::new().fg(t.label)));
    spans.push(span("cross-repo contract  ", Style::new().fg(t.muted)));
    spans.push(span(format!("{claim} "), Style::new().fg(t.label)));
    spans.push(span("file claim", Style::new().fg(t.muted)));
    clip(spans, width)
}

fn legend_shapes(app: &App, width: usize) -> Line<'static> {
    let t = &app.look.theme;
    let ascii = app.look.icons == IconSet::Ascii;
    let shapes: [(&str, Color, &str); 4] = if ascii {
        [
            ("o", t.text, "working"),
            ("#", t.ok, "verified"),
            ("||", t.warn, "held"),
            ("X", t.error, "blocked or escalated"),
        ]
    } else {
        [
            ("□", t.text, "working"),
            ("■", t.ok, "verified"),
            ("▌▐", t.warn, "held"),
            ("▚", t.error, "blocked or escalated"),
        ]
    };
    let mut spans = Vec::new();
    for (shape, color, text) in shapes {
        spans.push(span(format!("{shape} "), Style::new().fg(color)));
        spans.push(span(format!("{text}  "), Style::new().fg(t.muted)));
    }
    spans.push(span("one dot per configured check", Style::new().fg(t.dim)));
    clip(spans, width)
}

/// `spans` cut to `width` cells.
fn clip(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut used = 0;
    let mut out = Vec::new();
    for s in spans {
        let w = s.content.chars().count();
        if used + w > width {
            let keep = width.saturating_sub(used);
            if keep > 0 {
                out.push(Span::styled(fit(&s.content, keep), s.style));
            }
            break;
        }
        used += w;
        out.push(s);
    }
    Line::from(out)
}
