use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};

use super::{BLUE, MUTED, ORANGE, short, status_span};
use swarmux_domain::{Actor, JobProgress};

use crate::app::App;

const LANE_LABEL_W: usize = 10;

/// Below this height the guard panel is left out.
const MIN_HEIGHT_FOR_GUARD: u16 = 16;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let area = if area.height >= MIN_HEIGHT_FOR_GUARD {
        let [main, guard] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(super::guard::PANEL_HEIGHT),
        ])
        .areas(area);
        super::guard::render_panel(frame, guard, app);
        main
    } else {
        area
    };
    if let (false, Some(job)) = (app.sessions.is_empty(), app.current_job()) {
        render_job(frame, area, job, app.now_tick());
        return;
    }
    let hint = if app.sessions.is_empty() {
        "press 5 then n to create a session".to_string()
    } else {
        let target = app.selected_session().map_or("", |s| s.name.as_str());
        format!("press p to send a prompt to {target}")
    };
    let block = Block::bordered()
        .border_style(Style::new().fg(MUTED))
        .title(" dashboard ");
    frame.render_widget(Paragraph::new(hint).fg(MUTED).block(block), area);
}

/// Renders one job: status line, log tail (left) and timeline (right).
pub fn render_job(frame: &mut Frame, area: Rect, job: &JobProgress, now_tick: u32) {
    let [status, panes] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    frame.render_widget(status_line(job), status);

    let [log_area, timeline_area] =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(panes);

    let log_block = Block::bordered()
        .border_style(Style::new().fg(MUTED))
        .title(" tail -f session.log ");
    let visible = log_area.height.saturating_sub(2) as usize;
    let lines: Vec<Line> = job
        .log
        .iter()
        .skip(job.log.len().saturating_sub(visible))
        .map(|l| Line::raw(l.as_str()))
        .collect();
    frame.render_widget(Paragraph::new(lines).block(log_block), log_area);

    let tl_block = Block::bordered()
        .border_style(Style::new().fg(MUTED))
        .title(" session timeline ");
    let width = timeline_area.width.saturating_sub(2) as usize;
    let lines = timeline_lines(job, now_tick, width.saturating_sub(LANE_LABEL_W + 1));
    frame.render_widget(Paragraph::new(lines).block(tl_block), timeline_area);
}

fn status_line(job: &JobProgress) -> Line<'_> {
    let ctx = match job.context_window {
        Some(limit) => format!("ctx {}/{}", short(job.context_used), short(limit)),
        None => format!("ctx {}", short(job.context_used)),
    };
    Line::from(vec![
        Span::raw(format!(" #{} {} · ", job.job_id, job.session_name)),
        status_span(job.status),
        Span::raw(format!(
            " · {} · ${:.4} · in {} / out {} · {ctx}",
            job.model,
            job.cost_usd,
            short(job.tokens_in),
            short(job.tokens_out)
        )),
    ])
}

fn timeline_lines(job: &JobProgress, now_tick: u32, width: usize) -> Vec<Line<'_>> {
    let span = now_tick.max(1) as usize;
    job.lanes
        .iter()
        .map(|lane| {
            let mut row = vec![' '; width];
            if width > 0 {
                for &m in &lane.marks {
                    let x = (m as usize).min(span) * (width - 1) / span;
                    row[x] = '|';
                }
            }
            let color = if lane.actor == Actor::Lead {
                ORANGE
            } else {
                BLUE
            };
            let label: String = lane.label.chars().take(LANE_LABEL_W).collect();
            Line::from(vec![
                Span::styled(format!("{label:<LANE_LABEL_W$} "), Style::new().fg(color)),
                Span::styled(row.into_iter().collect::<String>(), Style::new().fg(color)),
            ])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use swarmux_domain::{JobId, Lane};

    fn render(job: &JobProgress) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal.draw(|f| render_job(f, f.area(), job, 50)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..20)
            .map(|y| (0..100).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn dashboard_renders_cost_and_lanes() {
        let mut job = JobProgress::new(JobId(1), "demo", "claude-sonnet");
        job.cost_usd = 0.5;
        job.context_used = 15_100;
        job.context_window = Some(1_000_000);
        job.lanes[0].marks = vec![0, 10, 50];
        job.lanes.push(Lane {
            actor: Actor::Subagent("toolu_A".into()),
            label: "explorer".into(),
            marks: vec![20],
        });
        job.log
            .push_back("00:01 lead       text        hello".into());
        let out = render(&job);
        assert!(out.contains("$0.5000"), "{out}");
        assert!(out.contains("ctx 15.1k/1.0M"), "{out}");
        assert!(out.contains("lead") && out.contains("explorer"), "{out}");
        assert!(out.contains("hello"), "{out}");
    }

    #[test]
    fn dashboard_empty_state_hint() {
        let t = crate::app::tests_support::test_app();
        let out = crate::ui::render_to_string(&t.app, 100, 10);
        assert!(out.contains("press 5 then n"), "{out}");
    }
}
