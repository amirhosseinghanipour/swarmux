//! The lead: the main Claude Code session that orchestrates the job.

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_domain::display_name;

use super::parts::{dim, label, status_mark, text, tokens};
use crate::theme::Theme;
use crate::ui::grid::chrome::{self, fingerprint};
use crate::ui::grid::{Panel, PanelId, ViewModel};
use crate::ui::look::Icon;
use crate::ui::widgets::bar_spans;
use crate::ui::{short, status_span};

pub struct Lead;

impl Panel for Lead {
    fn id(&self) -> PanelId {
        PanelId::Lead
    }
    fn title(&self) -> &'static str {
        "main session"
    }
    fn icon(&self) -> Icon {
        Icon::Lead
    }
    fn min_size(&self) -> (u16, u16) {
        (30, 3)
    }
    fn priority(&self) -> u8 {
        0
    }

    /// `<Model> · main session [spinner]`, its context and what caching saved.
    fn render(&self, frame: &mut Frame, area: Rect, vm: &ViewModel, t: &Theme, _tick: u64) {
        let app = vm.app;
        let Some(job) = vm.view.job else {
            let model = app.selected_session().map(|s| s.model.clone());
            let color = t.model(model.as_deref());
            let block = chrome::block(
                self,
                vm,
                t,
                color,
                vec![chrome::title(
                    format!(
                        " {} · main session ",
                        model.as_deref().map_or("—".into(), display_name)
                    ),
                    color,
                )],
                None,
            );
            let hint = match app.selected_session() {
                Some(s) => format!(
                    "no job yet · press {} to send a prompt to {}",
                    crate::hints::key_or_name(app, crate::keys::PROMPT_SEND),
                    s.name
                ),
                None => "no job yet · create a session first (5, then n)".into(),
            };
            frame.render_widget(Paragraph::new(Line::from(dim(t, hint))).block(block), area);
            return;
        };
        let color = t.model(Some(&job.model));
        let title = vec![
            chrome::title(
                format!(" {} · main session ", display_name(&job.model)),
                color,
            ),
            status_mark(t, job.status, vm.spin()),
            Span::raw(" "),
        ];
        let block = chrome::block(
            self,
            vm,
            t,
            color,
            title,
            Some(fingerprint((job.status.as_str(), job.context_used / 1000))),
        );
        let inner = block.inner(area);
        let mut lines = vec![
            Line::from(vec![
                label(t, "effort "),
                text(t, "—"),
                label(t, " · ctx "),
                text(t, context(job.context_used, job.context_window)),
                label(t, " · cost "),
                text(t, format!("${:.4}", job.cost_usd)),
                label(t, " · "),
                status_span(t, job.status),
            ]),
            Line::from(vec![
                label(t, "orchestrates · saved "),
                text(t, tokens(job.cache_read())),
                label(t, " tokens from cache · "),
                Span::styled(
                    format!("#{} {}", job.job_id, job.session_name),
                    Style::new().fg(t.muted),
                ),
            ]),
        ];
        if inner.height >= 3
            && let Some(window) = job.context_window.filter(|w| *w > 0)
        {
            let share = vm.tween("lead.ctx", job.context_used as f64 / window as f64);
            let w = (inner.width as usize).saturating_sub(9);
            let mut spans = vec![label(t, "ctx ")];
            spans.extend(bar_spans(share, w, &t.gauge, t.dim));
            spans.push(label(t, format!(" {:>3.0}%", share * 100.0)));
            lines.push(Line::from(spans));
        }
        frame.render_widget(block, area);
        frame.render_widget(Paragraph::new(lines), inner);
    }

    /// One line for small windows.
    fn summary(&self, vm: &ViewModel, t: &Theme, _tick: u64) -> Line<'static> {
        let Some(job) = vm.view.job else {
            return Line::from(dim(
                t,
                format!(
                    " no job yet · press {} to send a prompt",
                    crate::hints::key_or_name(vm.app, crate::keys::PROMPT_SEND)
                ),
            ));
        };
        let color = t.model(Some(&job.model));
        Line::from(vec![
            Span::styled(
                format!(" {} · main ", display_name(&job.model)),
                Style::new().fg(color),
            ),
            status_mark(t, job.status, vm.spin()),
            label(t, " ctx "),
            text(t, context(job.context_used, job.context_window)),
            label(t, " · saved "),
            text(t, tokens(job.cache_read())),
            label(t, " · "),
            text(t, format!("${:.4}", job.cost_usd)),
        ])
    }
}

/// `15.1k / 1.0M`, or `—` before the first turn.
pub fn context(used: u64, window: Option<u64>) -> String {
    match (used, window) {
        (0, _) => "—".into(),
        (u, Some(w)) => format!("{} / {}", short(u), short(w)),
        (u, None) => short(u),
    }
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
