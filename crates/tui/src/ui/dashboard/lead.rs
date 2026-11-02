//! The lead: the main Claude Code session that orchestrates the job.

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_domain::display_name;

use super::View;
use super::widgets::{dim, label, model_color, panel, status_mark, tokens};
use crate::app::App;
use crate::ui::{MUTED, short, status_span};

/// `<Model> · main session [spinner]`, its context and what caching saved.
pub fn panel_box(frame: &mut Frame, area: Rect, app: &App, view: &View) {
    let Some(job) = view.job else {
        let model = app.selected_session().map(|s| s.model.clone());
        let block = panel(
            format!(
                " {} · main session ",
                model.as_deref().map_or("—".into(), display_name)
            ),
            model_color(model.as_deref()),
        );
        let hint = match app.selected_session() {
            Some(s) => format!("no job yet · press p to send a prompt to {}", s.name),
            None => "no job yet · create a session first (5, then n)".into(),
        };
        frame.render_widget(Paragraph::new(Line::from(dim(hint))).block(block), area);
        return;
    };
    let color = model_color(Some(&job.model));
    let mut title = vec![Span::styled(
        format!(" {} · main session ", display_name(&job.model)),
        Style::new()
            .fg(color)
            .add_modifier(ratatui::style::Modifier::BOLD),
    )];
    title.push(status_mark(job.status, view.frame));
    title.push(Span::raw(" "));
    let block = panel(String::new(), color).title(Line::from(title));
    let lines = vec![
        Line::from(vec![
            label("effort "),
            Span::raw("—"),
            label(" · ctx "),
            Span::raw(context(job.context_used, job.context_window)),
            label(" · cost "),
            Span::raw(format!("${:.4}", job.cost_usd)),
            label(" · "),
            status_span(job.status),
        ]),
        Line::from(vec![
            label("orchestrates · saved "),
            Span::raw(tokens(job.cache_read())),
            label(" tokens from cache · "),
            Span::styled(
                format!("#{} {}", job.job_id, job.session_name),
                Style::new().fg(MUTED),
            ),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// `15.1k / 1.0M`, or `—` before the first turn.
pub fn context(used: u64, window: Option<u64>) -> String {
    match (used, window) {
        (0, _) => "—".into(),
        (u, Some(w)) => format!("{} / {}", short(u), short(w)),
        (u, None) => short(u),
    }
}

/// One line for small windows.
pub fn one_line(view: &View) -> Line<'static> {
    let Some(job) = view.job else {
        return Line::from(dim(" no job yet · press p to send a prompt"));
    };
    let color = model_color(Some(&job.model));
    Line::from(vec![
        Span::styled(
            format!(" {} · main ", display_name(&job.model)),
            Style::new().fg(color),
        ),
        status_mark(job.status, view.frame),
        label(" ctx "),
        Span::raw(context(job.context_used, job.context_window)),
        label(" · saved "),
        Span::raw(tokens(job.cache_read())),
        label(" · "),
        Span::raw(format!("${:.4}", job.cost_usd)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_text() {
        assert_eq!(context(0, Some(1)), "—");
        assert_eq!(context(15_100, Some(1_000_000)), "15.1k / 1.0M");
        assert_eq!(context(900, None), "900");
    }
}
