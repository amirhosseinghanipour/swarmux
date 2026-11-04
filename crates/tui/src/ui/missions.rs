//! The Missions screen: what outcomes are in flight and how they are
//! doing. Cards (state breadcrumb, acceptance chips, budget by phase,
//! topology) on the left; the selected contract, or the composer with the
//! lint's questions inline, on the right.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use swarmux_domain::gate::ALWAYS_PROTECTED;
use swarmux_domain::mission::{Mission, MissionPhase, MissionState, Severity, check_of};

use crate::app::App;
use crate::missions::{Composer, FIELDS};
use crate::theme::Theme;
use crate::ui::dashboard::{dim, fit, label};
use crate::ui::input::Input;
use crate::ui::look::IconSet;
use crate::ui::mission::{age, panel};

mod plan;

/// Rows of one card, borders included.
const CARD_H: u16 = 6;
/// From this width the contract sits beside the cards.
const WIDE: u16 = 120;
/// Longest budget bar, in cells.
const BAR_MAX: usize = 40;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    if area.width < 60 || area.height < 12 {
        let lines = vec![
            Line::from(label(t, "make the window bigger")).centered(),
            Line::from(dim(t, "missions need 80×24")).centered(),
        ];
        frame.render_widget(Paragraph::new(lines), area);
        return;
    }
    let [strip, body] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    frame.render_widget(strip_line(app), strip);
    let (cards, side) = if body.width >= WIDE {
        let [a, _, b] = Layout::horizontal([
            Constraint::Fill(3),
            Constraint::Length(1),
            Constraint::Fill(2),
        ])
        .areas(body);
        (a, b)
    } else {
        let side_h = (body.height / 2).clamp(8, 18);
        let [a, b] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(side_h)]).areas(body);
        (a, b)
    };
    render_cards(frame, cards, app);
    match &app.missions_screen.composer {
        Some(c) => render_composer(frame, side, app, c),
        None => render_contract(frame, side, app),
    }
}

/// A glyph from the Unicode set, or its ASCII stand-in.
fn g(app: &App, unicode: &'static str, ascii: &'static str) -> &'static str {
    if app.look.icons == IconSet::Ascii {
        ascii
    } else {
        unicode
    }
}

/// How a state looks: its mark and colour.
fn state_look(app: &App, state: MissionState) -> (&'static str, Color) {
    let t = &app.look.theme;
    match state {
        MissionState::Draft => (g(app, "·", "."), t.muted),
        MissionState::Linted => (g(app, "○", "o"), t.label),
        MissionState::Queued => (g(app, "…", "~"), t.muted),
        MissionState::Running => (g(app, "▶", ">"), t.lead),
        MissionState::Verifying => (g(app, "◎", "?"), t.warn),
        MissionState::Delivered => (g(app, "✓", "+"), t.ok),
        MissionState::Escalated => (g(app, "⚑", "!"), t.error),
        MissionState::Refused => (g(app, "⊘", "x"), t.error),
    }
}

/// `missions 4 · ▶ 1 running · ✓ 1 delivered …   spent $3.21 of $12.00`.
fn strip_line(app: &App) -> Line<'static> {
    let t = &app.look.theme;
    let all = app.missions();
    let mut spans = vec![label(t, format!(" missions {}", all.len()))];
    for state in MissionState::ALL {
        let n = all.iter().filter(|m| m.state == state).count();
        if n == 0 {
            continue;
        }
        let (mark, color) = state_look(app, state);
        spans.push(dim(t, "  "));
        spans.push(Span::styled(
            format!("{mark} {n} {state}"),
            Style::new().fg(color),
        ));
    }
    let spent: f64 = all.iter().map(|m| m.spend.total()).sum();
    let budgeted: f64 = all.iter().filter_map(|m| m.contract.budget.usd).sum();
    if !all.is_empty() {
        let money = if budgeted > 0.0 {
            format!("   spent ${spent:.2} of ${budgeted:.2} budgeted")
        } else {
            format!("   spent ${spent:.2}")
        };
        spans.push(dim(t, money));
    }
    Line::from(spans)
}

fn render_cards(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let all = app.missions();
    if all.is_empty() {
        let lines = vec![
            Line::default(),
            Line::from(label(t, "No missions yet.")).centered(),
            Line::default(),
            Line::from(dim(
                t,
                "A mission is an outcome, the checks that prove it, what the agent",
            ))
            .centered(),
            Line::from(dim(
                t,
                "must not touch and what it may cost. Press n to write one.",
            ))
            .centered(),
        ];
        frame.render_widget(Paragraph::new(lines), area);
        return;
    }
    let fits = usize::from((area.height / CARD_H).max(1));
    let selected = app.missions_screen.selected.min(all.len() - 1);
    let first = selected.saturating_sub(fits - 1);
    for (slot, (i, m)) in all.iter().enumerate().skip(first).take(fits).enumerate() {
        let rect = Rect {
            y: area.y + slot as u16 * CARD_H,
            height: CARD_H.min(area.bottom() - (area.y + slot as u16 * CARD_H)),
            ..area
        };
        render_card(frame, rect, app, m, i == selected);
    }
    if all.len() > fits {
        let more = format!(" {}/{} ", selected + 1, all.len());
        let at = Rect {
            x: area.right().saturating_sub(more.chars().count() as u16 + 1),
            y: area.bottom().saturating_sub(1),
            width: more.chars().count() as u16,
            height: 1,
        };
        frame.render_widget(Line::from(dim(t, more)), at);
    }
}

fn render_card(frame: &mut Frame, area: Rect, app: &App, m: &Mission, selected: bool) {
    let t = &app.look.theme;
    let (mark, color) = state_look(app, m.state);
    let title = if m.contract.title.is_empty() {
        m.slug.as_str()
    } else {
        m.contract.title.as_str()
    };
    let meta = Line::from(vec![
        Span::styled(format!(" {} ", m.state), Style::new().fg(color)),
        dim(t, format!("{} ", age(app.now() - m.updated_at))),
    ]);
    let border = if selected { t.border_focus } else { t.border };
    let block = panel(t, mark, &fit(title, 48), meta, border);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let w = usize::from(inner.width);
    let lines = vec![
        breadcrumb(app, m, w),
        chips(app, m, w),
        budget_bar(app, m, w),
        footer(app, m, w),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

/// `draft › linted › queued › [running] › verifying › delivered`; an
/// escalated or refused mission ends where it stopped.
fn breadcrumb(app: &App, m: &Mission, w: usize) -> Line<'static> {
    let t = &app.look.theme;
    let sep = g(app, " › ", " > ");
    let path = MissionState::PATH;
    let reached = match m.state {
        MissionState::Escalated if m.job.is_some() => 3,
        MissionState::Escalated | MissionState::Refused => 1,
        s => path.iter().position(|p| *p == s).unwrap_or(0),
    };
    let ended = m.state.is_final() && m.state != MissionState::Delivered;
    let mut spans = Vec::new();
    for (i, step) in path.iter().enumerate() {
        if ended && i > reached {
            break;
        }
        if i > 0 {
            spans.push(dim(t, sep));
        }
        let style = if i == reached && !ended {
            let (_, color) = state_look(app, *step);
            Style::new()
                .fg(t.on_accent)
                .bg(color)
                .add_modifier(Modifier::BOLD)
        } else if i < reached || ended {
            Style::new().fg(t.label)
        } else {
            Style::new().fg(t.dim)
        };
        spans.push(Span::styled(step.as_str().to_string(), style));
    }
    if ended {
        let (mark, color) = state_look(app, m.state);
        spans.push(dim(t, sep));
        spans.push(Span::styled(
            format!("{mark} {}", m.state),
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ));
    }
    cut_line(spans, w)
}

/// What the checks said, by check name: passed, flaky.
fn check_states(app: &App, m: &Mission) -> Vec<(String, bool, bool)> {
    m.job
        .and_then(|j| app.mission.gates.get(&j))
        .map(|g| g.checks.clone())
        .unwrap_or_default()
}

/// `✓ cargo test  ✗ bench  · lint   ⛨ 3 protected`.
fn chips(app: &App, m: &Mission, w: usize) -> Line<'static> {
    let t = &app.look.theme;
    let states = check_states(app, m);
    let mut spans = Vec::new();
    let checks = &m.contract.acceptance.checks;
    if checks.is_empty() {
        spans.push(Span::styled(
            format!("{} no acceptance checks  ", g(app, "✗", "x")),
            Style::new().fg(t.error),
        ));
    }
    for line in checks {
        let Some(check) = check_of(line) else {
            continue;
        };
        let (mark, color) = match states.iter().find(|(n, _, _)| *n == check.name) {
            Some((_, true, true)) => (g(app, "✓", "+"), t.warn),
            Some((_, true, false)) => (g(app, "✓", "+"), t.ok),
            Some((_, false, _)) => (g(app, "✗", "x"), t.error),
            None => (g(app, "·", "."), t.muted),
        };
        spans.push(Span::styled(format!("{mark} "), Style::new().fg(color)));
        spans.push(Span::styled(
            format!("{}  ", fit(&check.name, 28)),
            Style::new().fg(t.text),
        ));
    }
    let protected = m.contract.constraints.protected.len();
    spans.push(Span::styled(
        format!("{} {protected} protected", g(app, "⛨", "#")),
        Style::new().fg(if protected == 0 { t.warn } else { t.guard }),
    ));
    if m.contract.acceptance.human_signoff {
        spans.push(dim(t, format!("  {} sign-off", g(app, "☐", "[ ]"))));
    }
    cut_line(spans, w)
}

/// Colour of a phase's part of the budget bar.
fn phase_color(t: &Theme, phase: MissionPhase) -> Color {
    match phase {
        MissionPhase::Plan => t.haiku,
        MissionPhase::Implement => t.lead,
        MissionPhase::Verify => t.ok,
        MissionPhase::Review => t.reviewer,
    }
}

/// The budget split by phase, each part filled with what it spent.
fn budget_bar(app: &App, m: &Mission, w: usize) -> Line<'static> {
    let t = &app.look.theme;
    let spent = m.spend.total();
    let mut spans = vec![label(t, "budget ")];
    let Some(usd) = m.contract.budget.usd else {
        spans.push(dim(t, format!("spent ${spent:.2} · no budget set")));
        return cut_line(spans, w);
    };
    let tail = format!(" ${spent:.2} of ${usd:.2}");
    let bar_w = w.saturating_sub(7 + tail.chars().count() + 2).min(BAR_MAX);
    let (full, empty) = (g(app, "█", "#"), g(app, "░", "-"));
    let mut used = 0;
    for (i, (phase, share)) in MissionPhase::SHARES.iter().enumerate() {
        let cells = if i + 1 == MissionPhase::SHARES.len() {
            bar_w.saturating_sub(used)
        } else {
            ((bar_w as f64) * share).round() as usize
        };
        used += cells;
        let room = usd * share;
        let ratio = (m.spend.of(*phase) / room).clamp(0.0, 1.0);
        let filled = ((cells as f64) * ratio).ceil() as usize;
        let color = phase_color(t, *phase);
        spans.push(Span::styled(full.repeat(filled), Style::new().fg(color)));
        spans.push(Span::styled(
            empty.repeat(cells.saturating_sub(filled)),
            Style::new().fg(t.dim),
        ));
    }
    let over = spent > usd;
    spans.push(Span::styled(
        tail,
        Style::new().fg(if over { t.error } else { t.text }),
    ));
    if let Some(mins) = m.contract.budget.wall_minutes {
        spans.push(dim(t, format!(" · {mins}m")));
    }
    cut_line(spans, w)
}

/// Topology, the job, open questions and the lint, or why it stopped.
fn footer(app: &App, m: &Mission, w: usize) -> Line<'static> {
    let t = &app.look.theme;
    let topology = match app.mission_plan(&m.slug).and_then(|r| r.graph.as_ref()) {
        Some(run) => format!("{} task graph ×{}", g(app, "◇", "<>"), run.tasks.len()),
        None => format!("{} single session", g(app, "◇", "<>")),
    };
    let mut spans = vec![Span::styled(topology, Style::new().fg(t.label))];
    if let Some(job) = m.job {
        let session = m
            .session
            .and_then(|id| app.sessions.iter().find(|s| s.id == id))
            .map_or_else(String::new, |s| format!(" {}", s.name));
        spans.push(dim(t, format!(" · job #{job}{session}")));
    }
    if let Some(note) = &m.note {
        spans.push(Span::styled(format!(" · {note}"), Style::new().fg(t.error)));
        return cut_line(spans, w);
    }
    if app.mission_linting(&m.slug) {
        spans.push(Span::styled(" · linting…", Style::new().fg(t.haiku)));
    }
    if let Some((tag, loud)) = plan::card_tag(app, m) {
        let color = if loud { t.warn } else { t.muted };
        spans.push(Span::styled(tag, Style::new().fg(color)));
    }
    let open = m
        .contract
        .clarifications
        .iter()
        .filter(|q| q.answer.trim().is_empty())
        .count();
    if open > 0 && !m.state.in_flight() && !m.state.is_final() {
        spans.push(Span::styled(
            format!(" · {open} question{}", if open == 1 { "" } else { "s" }),
            Style::new().fg(t.warn),
        ));
    }
    if let Some(lint) = &m.lint {
        let count = |s: Severity| lint.findings.iter().filter(|f| f.severity == s).count();
        let (blocking, warnings) = (count(Severity::Blocking), count(Severity::Warning));
        if blocking > 0 {
            spans.push(Span::styled(
                format!(" · {blocking} blocking"),
                Style::new().fg(t.error),
            ));
        }
        if warnings > 0 {
            spans.push(Span::styled(
                format!(
                    " · {warnings} warning{}",
                    if warnings == 1 { "" } else { "s" }
                ),
                Style::new().fg(t.warn),
            ));
        }
    }
    cut_line(spans, w)
}

/// The spans, cut to `w` columns with `…`.
fn cut_line(spans: Vec<Span<'static>>, w: usize) -> Line<'static> {
    let mut out = Vec::new();
    let mut used = 0;
    for s in spans {
        let n = s.content.chars().count();
        if used + n <= w {
            used += n;
            out.push(s);
            continue;
        }
        let room = w.saturating_sub(used);
        if room > 0 {
            out.push(Span::styled(fit(&s.content, room), s.style));
        }
        break;
    }
    Line::from(out)
}

fn heading(t: &Theme, s: &str) -> Line<'static> {
    Line::from(Span::styled(
        s.to_string(),
        Style::new().fg(t.title).add_modifier(Modifier::BOLD),
    ))
}

fn severity_span(app: &App, s: Severity) -> Span<'static> {
    let t = &app.look.theme;
    match s {
        Severity::Blocking => {
            Span::styled(format!("{} ", g(app, "✗", "x")), Style::new().fg(t.error))
        }
        Severity::Warning => {
            Span::styled(format!("{} ", g(app, "⚠", "!")), Style::new().fg(t.warn))
        }
        Severity::Info => Span::styled(format!("{} ", g(app, "·", ".")), Style::new().fg(t.muted)),
    }
}

/// The lint's findings and the questions, for the contract and composer.
fn lint_lines(app: &App, m: &Mission, composer: Option<&Composer>) -> Vec<Line<'static>> {
    let t = &app.look.theme;
    let mut out = Vec::new();
    let Some(lint) = &m.lint else {
        return out;
    };
    let source = if app.mission_linting(&m.slug) {
        format!("lint · {} · asking haiku…", lint.source)
    } else {
        format!("lint · {}", lint.source)
    };
    out.push(heading(t, &source));
    if lint.findings.is_empty() {
        out.push(Line::from(dim(t, "  nothing found")));
    }
    for f in &lint.findings {
        out.push(Line::from(vec![
            Span::raw("  "),
            severity_span(app, f.severity),
            Span::styled(f.detail.clone(), Style::new().fg(t.text)),
        ]));
    }
    if let Some(p) = &lint.problem {
        out.push(Line::from(dim(
            t,
            format!("  the model could not be asked: {p}"),
        )));
    }
    if m.contract.clarifications.is_empty() {
        return out;
    }
    out.push(Line::default());
    out.push(heading(t, "questions"));
    for (i, q) in m.contract.clarifications.iter().enumerate() {
        out.push(Line::from(vec![
            Span::styled(format!("  {} ", g(app, "?", "?")), Style::new().fg(t.warn)),
            Span::styled(q.question.clone(), Style::new().fg(t.text)),
        ]));
        let answer = match composer {
            Some(c) => {
                let focused = c.answering() == Some(i);
                let input = c.answers.get(i).cloned().unwrap_or_default();
                input_spans(t, "    → ", &input, focused)
            }
            None if q.answer.trim().is_empty() => {
                vec![dim(t, "    unanswered · e answers it")]
            }
            None => vec![
                dim(t, "    → "),
                Span::styled(q.answer.clone(), Style::new().fg(t.label)),
            ],
        };
        out.push(Line::from(answer));
    }
    out
}

fn input_spans(t: &Theme, prefix: &str, input: &Input, focused: bool) -> Vec<Span<'static>> {
    let lead = Span::styled(
        prefix.to_string(),
        Style::new().fg(if focused { t.lead } else { t.muted }),
    );
    if !focused {
        return vec![
            lead,
            Span::styled(input.text.clone(), Style::new().fg(t.text)),
        ];
    }
    let (before, at, after) = input.split_at_cursor();
    vec![
        lead,
        Span::raw(before),
        Span::styled(at, Style::new().add_modifier(Modifier::REVERSED)),
        Span::raw(after),
    ]
}

/// The selected mission's contract in full.
fn render_contract(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let all = app.missions();
    let Some(m) = all.get(
        app.missions_screen
            .selected
            .min(all.len().saturating_sub(1)),
    ) else {
        let block = panel(t, g(app, "◇", "<>"), "contract", Line::default(), t.border);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let lines = vec![
            heading(t, "an outcome contract"),
            Line::from(dim(t, "goal       what should be true when it is done")),
            Line::from(dim(
                t,
                "checks     commands that prove it; the gate runs them",
            )),
            Line::from(dim(t, "work in    where the agent should work")),
            Line::from(dim(
                t,
                "protect    what it may never change: the tests it is judged by",
            )),
            Line::from(dim(t, "budget     dollars and minutes: $2 90m")),
        ];
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        return;
    };
    let block = panel(
        t,
        g(app, "◇", "<>"),
        "contract",
        Line::from(dim(t, format!(" .swarmux/missions/{}.toml ", m.slug))),
        t.border,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let c = &m.contract;
    let mut lines = vec![
        heading(t, "goal"),
        Line::from(Span::styled(c.goal.clone(), Style::new().fg(t.text))),
        Line::default(),
        heading(t, "done when the gate passes"),
    ];
    let states = check_states(app, m);
    for line in &c.acceptance.checks {
        let name = check_of(line).map(|c| c.name).unwrap_or_default();
        let mark = match states.iter().find(|(n, _, _)| *n == name) {
            Some((_, true, _)) => {
                Span::styled(format!("  {} ", g(app, "✓", "+")), Style::new().fg(t.ok))
            }
            Some((_, false, _)) => {
                Span::styled(format!("  {} ", g(app, "✗", "x")), Style::new().fg(t.error))
            }
            None => dim(t, format!("  {} ", g(app, "·", "."))),
        };
        lines.push(Line::from(vec![
            mark,
            Span::styled(line.clone(), Style::new().fg(t.text)),
        ]));
    }
    if c.acceptance.checks.is_empty() {
        lines.push(Line::from(dim(
            t,
            "  no checks: nothing can tell it is done",
        )));
    }
    let mut protected = c.constraints.protected.clone();
    protected.push(ALWAYS_PROTECTED.to_string());
    lines.push(Line::from(vec![
        label(t, "work in    "),
        Span::styled(
            if c.constraints.paths_rw.is_empty() {
                "anywhere".to_string()
            } else {
                c.constraints.paths_rw.join(", ")
            },
            Style::new().fg(t.text),
        ),
    ]));
    lines.push(Line::from(vec![
        label(t, "protected  "),
        Span::styled(protected.join(", "), Style::new().fg(t.guard)),
    ]));
    let shares: Vec<String> = MissionPhase::SHARES
        .iter()
        .map(|(p, s)| format!("{} {:.0}%", p.as_str(), s * 100.0))
        .collect();
    lines.push(Line::from(vec![
        label(t, "budget     "),
        dim(t, shares.join(" · ")),
    ]));
    lines.extend(plan::plan_lines(app, m));
    lines.push(Line::default());
    lines.extend(lint_lines(app, m, None));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_composer(frame: &mut Frame, area: Rect, app: &App, c: &Composer) {
    let t = &app.look.theme;
    let title = match &c.slug {
        Some(slug) => format!("mission · {slug}"),
        None => "new mission".to_string(),
    };
    let block = panel(
        t,
        g(app, "✎", "*"),
        &title,
        Line::from(dim(t, " ctrl+s lint · ctrl+r run ")),
        t.border_focus,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines = Vec::new();
    for (i, name) in FIELDS.iter().enumerate() {
        let prefix = format!("{name:<9}");
        lines.push(Line::from(input_spans(
            t,
            &prefix,
            &c.inputs[i],
            c.focus == i,
        )));
    }
    lines.push(Line::from(dim(
        t,
        "checks split by ;   paths by ,   budget like $2 90m",
    )));
    lines.push(Line::default());
    let saved = c
        .slug
        .as_deref()
        .and_then(|s| app.missions().iter().find(|m| m.slug == s));
    match saved {
        Some(m) => lines.extend(lint_lines(app, m, Some(c))),
        None => lines.push(Line::from(dim(
            t,
            "ctrl+s saves it and lints it: the questions appear here",
        ))),
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

#[cfg(test)]
mod tests;
