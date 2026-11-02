//! The dashboard's status line: step, effort, subagents, lead, reviewer, guard.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use swarmux_domain::{JobStatus, ReviewState};

use super::View;
use super::guard::thousands;
use super::widgets::label;
use crate::app::App;
use crate::ui::{LABEL, short};

/// `| step [plan] effort [—] subagents [2/3 active] lead [ctx 15.1k] …`
pub fn line(app: &App, view: &View) -> Line<'static> {
    let value = |s: String| Span::styled(s, Style::new().add_modifier(Modifier::BOLD));
    let pair = |name: &str, v: String| [label(format!(" {name} [")), value(v), label("]")];
    let job = view.job;
    let step = job.map_or("—".into(), |j| j.phase().as_str().to_string());
    let subagents = job.map_or("—".into(), |j| {
        format!("{}/{} active", j.subagents_running(), j.subagents.len())
    });
    let ctx = job
        .filter(|j| j.context_used > 0)
        .map_or("—".into(), |j| format!("ctx {}", short(j.context_used)));
    let reviewer = match job.and_then(|j| j.review.as_ref()) {
        None if job.is_none() && app.review_next() => "on call".to_string(),
        None => "off".into(),
        Some(ReviewState::Requested) => "on call".into(),
        Some(ReviewState::Running { .. }) => match view.review.map(|r| r.status) {
            Some(JobStatus::Running) => "reviewing".into(),
            _ => "queued".into(),
        },
        Some(ReviewState::Done { review, .. }) if review.concerns.is_empty() => "passed".into(),
        Some(ReviewState::Done { review, .. }) => format!("{} concerns", review.concerns.len()),
        Some(ReviewState::Failed { .. }) => "failed".into(),
    };
    let decisions = format!("{} decisions", thousands(app.guard.stats().total()));
    let mut spans = vec![Span::styled(" |", Style::new().fg(LABEL))];
    spans.extend(pair("step", step));
    spans.extend(pair("effort", "—".into()));
    spans.extend(pair("subagents", subagents));
    spans.extend(pair("lead", ctx));
    spans.extend(pair("reviewer", reviewer));
    spans.extend(pair("guard", decisions));
    Line::from(spans)
}
