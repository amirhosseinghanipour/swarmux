//! The top of the dashboard: who orchestrates and audits, the legend, the
//! running command, the step tabs and where the next prompt goes.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use swarmux_domain::{Family, Phase, ReviewState, display_name};

use super::View;
use super::widgets::{dim, fit, label, model_color};
use crate::app::App;
use crate::ui::{BG, DIM, GREEN, LABEL, MUTED, ORANGE};

/// `SWARMUX · OPUS 5.5 ORCHESTRATES · OPUS AUDITS`, centered.
pub fn title(app: &App, view: &View) -> Line<'static> {
    let bold = |s: String, color: Color| {
        Span::styled(s, Style::new().fg(color).add_modifier(Modifier::BOLD))
    };
    let lead = match view.job {
        Some(job) => job.model.clone(),
        None => app
            .selected_session()
            .map(|s| s.model.clone())
            .unwrap_or_default(),
    };
    let mut spans = vec![bold("SWARMUX".into(), Color::White), label(" · ")];
    if !lead.is_empty() {
        spans.push(bold(
            display_name(&lead).to_uppercase(),
            model_color(Some(&lead)),
        ));
        spans.push(bold(" ORCHESTRATES".into(), Color::White));
        spans.push(label(" · "));
    }
    match reviewer_model(app, view) {
        Some(model) => {
            spans.push(bold(
                display_name(&model).to_uppercase(),
                model_color(Some(&model)),
            ));
            spans.push(bold(" AUDITS".into(), Color::White));
        }
        None => spans.push(label("REVIEW OFF (v)")),
    }
    Line::from(spans).centered()
}

/// The reviewer's model for this job: its run's, else the configured one
/// when the job (or, without a job, the next one) gets a review.
pub fn reviewer_model(app: &App, view: &View) -> Option<String> {
    if let Some(run) = view.review {
        return Some(run.model.clone());
    }
    let wanted = match view.job {
        Some(job) => job.review.is_some(),
        None => app.review_next(),
    };
    wanted.then(|| app.review_model().to_string())
}

/// `■ opus lead  ■ sonnet builder  ■ haiku 340 t/s  ■ guard  ■ reviewer`,
/// listing only what the current job has.
pub fn legend(view: &View) -> Line<'static> {
    let Some(job) = view.job else {
        return Line::from(dim("no job yet")).centered();
    };
    let mut spans = Vec::new();
    let mut entry = |color: Color, text: String| {
        spans.push(Span::styled("■ ", Style::new().fg(color)));
        spans.push(label(text));
        spans.push(Span::raw("   "));
    };
    let lead_family = Family::of(&job.model);
    entry(
        model_color(Some(&job.model)),
        format!("{} lead", lead_family.as_str()),
    );
    let mut builders: Vec<Family> = Vec::new();
    for f in job
        .subagents
        .iter()
        .filter_map(|s| s.model.as_deref().map(Family::of))
    {
        if f != Family::Haiku && !builders.contains(&f) {
            builders.push(f);
        }
    }
    for f in builders {
        entry(ORANGE, format!("{} builder", f.as_str()));
    }
    // Haiku throughput: output tokens per second of every Haiku actor.
    let mut haiku = Vec::new();
    if lead_family == Family::Haiku {
        haiku.push(job.lead_tokens.rate(Some(0)));
    }
    haiku.extend(
        job.subagents
            .iter()
            .filter(|s| s.model.as_deref().map(Family::of) == Some(Family::Haiku))
            .map(|s| s.rate()),
    );
    if let Some(r) = view
        .review
        .filter(|r| Family::of(&r.model) == Family::Haiku)
    {
        haiku.push(r.lead_tokens.rate(Some(0)));
    }
    if !haiku.is_empty() {
        let best = haiku
            .into_iter()
            .flatten()
            .fold(None, |a: Option<f64>, r| Some(a.map_or(r, |a| a.max(r))));
        let rate = best.map_or("—".into(), |r| format!("{r:.0}"));
        entry(crate::ui::BLUE, format!("haiku {rate} t/s"));
    }
    if !job.decision_marks.is_empty() {
        entry(GREEN, "guard".into());
    }
    if job.review.is_some() {
        let color = match view.review.map(|r| model_color(Some(&r.model))) {
            Some(c) if c != MUTED => c,
            _ => LABEL,
        };
        entry(color, "reviewer".into());
    }
    spans.pop();
    Line::from(spans).centered()
}

/// `> claude -p … --model … --resume …` and, on the right,
/// `3 subagents mounted · reviewer on call`.
pub fn command(frame: &mut Frame, area: Rect, view: &View) {
    let right = match view.job {
        Some(job) => {
            let reviewer = match &job.review {
                None => "no reviewer",
                Some(ReviewState::Requested | ReviewState::Running { .. }) => "reviewer on call",
                Some(ReviewState::Done { .. }) => "reviewer done",
                Some(ReviewState::Failed { .. }) => "review failed",
            };
            format!("{} subagents mounted · {reviewer} ", job.subagents.len())
        }
        None => String::new(),
    };
    let right_w = (right.chars().count() as u16).min(area.width / 2);
    let [left, right_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_w)]).areas(area);
    let cmd = view.job.and_then(|j| j.command.as_deref()).unwrap_or("—");
    let max = left.width.saturating_sub(3) as usize;
    frame.render_widget(
        Line::from(vec![
            Span::styled(" > ", Style::new().fg(ORANGE).add_modifier(Modifier::BOLD)),
            Span::styled(fit(cmd, max), Style::new().fg(MUTED)),
        ]),
        left,
    );
    frame.render_widget(
        Line::from(label(fit(&right, right_w as usize))).right_aligned(),
        right_area,
    );
}

/// `1 plan · 2 fork · … · 7 ship` with the current step as `> plan_`, and
/// key hints on the right.
pub fn steps(frame: &mut Frame, area: Rect, view: &View, hints: &str) {
    let current = view.job.map(|j| j.phase());
    let mut spans = vec![Span::raw(" ")];
    for (i, phase) in Phase::ALL.iter().enumerate() {
        if i > 0 {
            spans.push(dim(" · "));
        }
        let reached = view
            .job
            .is_some_and(|j| j.phase_ticks[phase.index()].is_some());
        if Some(*phase) == current {
            spans.push(Span::styled(
                format!(" > {}_ ", phase.as_str()),
                Style::new().fg(BG).bg(ORANGE).add_modifier(Modifier::BOLD),
            ));
        } else {
            let color = if reached { LABEL } else { DIM };
            spans.push(Span::styled(
                format!("{} {}", i + 1, phase.as_str()),
                Style::new().fg(color),
            ));
        }
    }
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let room = (area.width as usize).saturating_sub(used + 2);
    frame.render_widget(Line::from(spans), area);
    if room > 12 {
        let hint = fit(hints.trim(), room);
        frame.render_widget(Line::from(dim(format!("{hint} "))).right_aligned(), area);
    }
}

/// Where the next prompt goes, highlighted so it can't be missed.
pub fn target(frame: &mut Frame, area: Rect, app: &App) {
    let Some(s) = app.selected_session() else {
        frame.render_widget(
            Line::from(vec![
                Span::styled(" ▶ ", Style::new().fg(ORANGE)),
                label("no session yet: press 5 then n to create one in this folder"),
            ]),
            area,
        );
        return;
    };
    let mut spans = vec![
        Span::styled(" ▶ prompts go to ", Style::new().fg(LABEL)),
        Span::styled(
            format!(" {} ", s.name),
            Style::new().fg(BG).bg(ORANGE).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {} ", s.model),
            Style::new().fg(model_color(Some(&s.model))),
        ),
    ];
    if s.lean {
        spans.push(Span::styled("lean ", Style::new().fg(GREEN)));
    }
    let review = if app.review_next() {
        Span::styled("· review on ", Style::new().fg(GREEN))
    } else {
        dim("· review off ")
    };
    spans.push(review);
    spans.push(dim("· "));
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let tail = "   ↑↓ change";
    let room = (area.width as usize).saturating_sub(used + tail.len());
    spans.push(Span::styled(
        fit(&s.cwd.display().to_string(), room),
        Style::new().fg(MUTED),
    ));
    if room > 0 {
        spans.push(dim(tail));
    }
    frame.render_widget(Line::from(spans), area);
}
