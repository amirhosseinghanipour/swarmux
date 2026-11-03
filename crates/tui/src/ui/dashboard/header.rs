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
use super::parts::{dim, fit, label};
use crate::app::App;
use crate::theme::Theme;
use crate::ui::grid::{Panel, PanelId, ViewModel};
use crate::ui::look::Icon;

/// `SWARMUX · OPUS 5.5 ORCHESTRATES · OPUS AUDITS`, centered.
pub fn title(app: &App, view: &View, t: &Theme) -> Line<'static> {
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
    let mut spans = vec![bold("SWARMUX".into(), t.title), label(t, " · ")];
    if !lead.is_empty() {
        spans.push(bold(
            display_name(&lead).to_uppercase(),
            t.model(Some(&lead)),
        ));
        spans.push(bold(" ORCHESTRATES".into(), t.title));
        spans.push(label(t, " · "));
    }
    match reviewer_model(app, view) {
        Some(model) => {
            spans.push(bold(
                display_name(&model).to_uppercase(),
                t.model(Some(&model)),
            ));
            spans.push(bold(" AUDITS".into(), t.title));
        }
        None => spans.push(label(t, "REVIEW OFF (v)")),
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
pub fn legend(view: &View, t: &Theme) -> Line<'static> {
    let Some(job) = view.job else {
        return Line::from(dim(t, "no job yet")).centered();
    };
    let mut spans = Vec::new();
    let mut entry = |color: Color, text: String| {
        spans.push(Span::styled("■ ", Style::new().fg(color)));
        spans.push(label(t, text));
        spans.push(Span::raw("   "));
    };
    let lead_family = Family::of(&job.model);
    entry(
        t.model(Some(&job.model)),
        format!("{} lead", lead_family.as_str()),
    );
    let mut builders: Vec<(Family, Color)> = Vec::new();
    for s in &job.subagents {
        let Some(model) = s.model.as_deref() else {
            continue;
        };
        let f = Family::of(model);
        if f != Family::Haiku && !builders.iter().any(|(b, _)| *b == f) {
            builders.push((f, t.model(Some(model))));
        }
    }
    for (f, color) in builders {
        entry(color, format!("{} builder", f.as_str()));
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
        entry(t.haiku, format!("haiku {rate} t/s"));
    }
    if !job.decision_marks.is_empty() {
        entry(t.guard, "guard".into());
    }
    if job.review.is_some() {
        let color = view.review.map_or(t.reviewer, |r| t.model(Some(&r.model)));
        entry(color, "reviewer".into());
    }
    spans.pop();
    Line::from(spans).centered()
}

/// `> claude -p … --model … --resume …` and, on the right,
/// `3 subagents mounted · reviewer on call`.
pub fn command(frame: &mut Frame, area: Rect, view: &View, t: &Theme) {
    let right = match view.job {
        Some(job) => {
            let reviewer = match &job.review {
                None => "no reviewer",
                Some(ReviewState::Requested | ReviewState::Running { .. }) => "reviewer on call",
                Some(ReviewState::Done { .. }) => "reviewer done",
                Some(ReviewState::Failed { .. }) => "review failed",
                Some(ReviewState::Skipped { .. }) => "review skipped",
            };
            format!("{} subagents mounted · {reviewer} ", job.subagents.len())
        }
        None => String::new(),
    };
    let right_w = (right.chars().count() as u16).min(area.width / 2);
    let [left, right_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_w)]).areas(area);
    frame.render_widget(command_line(view, t, left.width), left);
    frame.render_widget(
        Line::from(label(t, fit(&right, right_w as usize))).right_aligned(),
        right_area,
    );
}

fn command_line(view: &View, t: &Theme, width: u16) -> Line<'static> {
    let cmd = view.job.and_then(|j| j.command.as_deref()).unwrap_or("—");
    let max = width.saturating_sub(3) as usize;
    Line::from(vec![
        Span::styled(" > ", Style::new().fg(t.lead).add_modifier(Modifier::BOLD)),
        Span::styled(fit(cmd, max), Style::new().fg(t.muted)),
    ])
}

/// `1 plan · 2 fork · … · 7 ship` with the current step as `> plan_`.
fn steps_line(view: &View, t: &Theme) -> Line<'static> {
    let current = view.job.map(|j| j.phase());
    let mut spans = vec![Span::raw(" ")];
    for (i, phase) in Phase::ALL.iter().enumerate() {
        if i > 0 {
            spans.push(dim(t, " · "));
        }
        let reached = view
            .job
            .is_some_and(|j| j.phase_ticks[phase.index()].is_some());
        if Some(*phase) == current {
            spans.push(Span::styled(
                format!(" > {}_ ", phase.as_str()),
                Style::new()
                    .fg(t.on_accent)
                    .bg(t.lead)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            let color = if reached { t.label } else { t.dim };
            spans.push(Span::styled(
                format!("{} {}", i + 1, phase.as_str()),
                Style::new().fg(color),
            ));
        }
    }
    Line::from(spans)
}

/// The step tabs and, when there is room, key hints on the right.
pub fn steps(frame: &mut Frame, area: Rect, view: &View, hints: &str, t: &Theme) {
    let line = steps_line(view, t);
    let used = line.width();
    let room = (area.width as usize).saturating_sub(used + 2);
    frame.render_widget(line, area);
    if room > 12 {
        let hint = fit(hints.trim(), room);
        frame.render_widget(Line::from(dim(t, format!("{hint} "))).right_aligned(), area);
    }
}

/// Where the next prompt goes, highlighted so it can't be missed.
pub fn target(app: &App, t: &Theme, width: u16) -> Line<'static> {
    let Some(s) = app.selected_session() else {
        return Line::from(vec![
            Span::styled(" ▶ ", Style::new().fg(t.lead)),
            label(
                t,
                "no session yet: press 6 then n to create one in this folder",
            ),
        ]);
    };
    let mut spans = vec![
        label(t, " ▶ prompts go to "),
        Span::styled(
            format!(" {} ", s.name),
            Style::new()
                .fg(t.on_accent)
                .bg(t.lead)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {} ", s.model),
            Style::new().fg(t.model(Some(&s.model))),
        ),
    ];
    if s.lean {
        spans.push(Span::styled("lean ", Style::new().fg(t.ok)));
    }
    spans.push(if app.review_next() {
        Span::styled("· review on ", Style::new().fg(t.ok))
    } else {
        dim(t, "· review off ")
    });
    spans.push(dim(t, "· "));
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let tail = "   ↑↓ change";
    let room = (width as usize).saturating_sub(used + tail.len());
    spans.push(Span::styled(
        fit(&s.cwd.display().to_string(), room),
        Style::new().fg(t.muted),
    ));
    if room > 0 {
        spans.push(dim(t, tail));
    }
    Line::from(spans)
}

/// A one-line strip of the header.
pub struct Strip(pub PanelId);

impl Panel for Strip {
    fn id(&self) -> PanelId {
        self.0
    }
    fn title(&self) -> &'static str {
        match self.0 {
            PanelId::Legend => "legend",
            PanelId::Command => "command",
            PanelId::Steps => "steps",
            _ => "target",
        }
    }
    fn icon(&self) -> Icon {
        Icon::Steps
    }
    fn min_size(&self) -> (u16, u16) {
        (1, 1)
    }
    fn priority(&self) -> u8 {
        match self.0 {
            PanelId::Steps => 1,
            PanelId::Target => 4,
            PanelId::Command => 6,
            _ => 7,
        }
    }
    fn boxed(&self) -> bool {
        false
    }
    fn render(&self, frame: &mut Frame, area: Rect, vm: &ViewModel, t: &Theme, _tick: u64) {
        let area = Rect { height: 1, ..area };
        match self.0 {
            PanelId::Legend => frame.render_widget(legend(&vm.view, t), area),
            PanelId::Command => command(frame, area, &vm.view, t),
            PanelId::Steps => steps(frame, area, &vm.view, crate::ui::hints(vm.app), t),
            _ => frame.render_widget(target(vm.app, t, area.width), area),
        }
    }
    fn summary(&self, vm: &ViewModel, t: &Theme, _tick: u64) -> Line<'static> {
        match self.0 {
            PanelId::Legend => legend(&vm.view, t),
            PanelId::Command => command_line(&vm.view, t, 200),
            PanelId::Steps => steps_line(&vm.view, t),
            _ => target(vm.app, t, 80),
        }
    }
}
