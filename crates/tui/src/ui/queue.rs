use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Style, Stylize},
    text::Span,
    widgets::{Cell, Paragraph, Row, Table, TableState},
};

use super::status_span;
use crate::app::App;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let block = super::rounded(t)
        .border_style(Style::new().fg(t.muted))
        .title(" queue ");
    let rows = app.queue_rows();
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new("no jobs yet: press 1 then p to send a prompt")
                .fg(t.muted)
                .block(block),
            area,
        );
        return;
    }
    let table_rows: Vec<Row> = rows
        .iter()
        .map(|e| {
            let priority = match e.priority {
                0 => String::new(),
                p => format!("{p:+}"),
            };
            Row::new(vec![
                Cell::from(format!("#{}", e.job_id)),
                Cell::from(status_span(t, e.status)),
                Cell::from(Span::styled(priority, Style::new().fg(t.muted))),
                Cell::from(e.session_name.clone()),
                Cell::from(format!("${:.4}", e.cost_usd)),
                Cell::from(e.prompt.lines().next().unwrap_or_default().to_string()),
            ])
        })
        .collect();
    let widths = [
        Constraint::Length(6),
        Constraint::Length(12),
        Constraint::Length(4),
        Constraint::Length(18),
        Constraint::Length(9),
        Constraint::Fill(1),
    ];
    let header = Row::new(["job", "status", "pri", "session", "cost", "prompt"]).fg(t.muted);
    let table = Table::new(table_rows, widths)
        .header(header)
        .block(block)
        .highlight_symbol("› ")
        .row_highlight_style(Style::new().bold());
    let mut state = TableState::default().with_selected(Some(app.queue.index(&rows)));
    frame.render_stateful_widget(table, area, &mut state);
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use swarmux_application::ports::RunnerMsg;
    use swarmux_domain::{AgentEvent, JobStatus};

    use crate::app::tests_support::{TestApp, test_app};
    use crate::app::{App, Screen};
    use crate::ui::render_to_string;

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    /// A session with one running and one queued job, on the queue screen.
    fn busy() -> TestApp {
        let mut t = test_app();
        t.with_session("demo");
        t.send("first job");
        t.send("second job");
        press(&mut t.app, KeyCode::Char('4'));
        t
    }

    #[test]
    fn empty_queue_hint() {
        let mut t = test_app();
        t.app.screen = Screen::Queue;
        let out = render_to_string(&t.app, 100, 8);
        assert!(out.contains("no jobs yet"), "{out}");
        assert!(out.contains("c cancel"), "{out}");
    }

    #[test]
    fn lists_running_then_queued_with_counts() {
        let t = busy();
        let out = render_to_string(&t.app, 110, 10);
        let running = out.find("running  ").unwrap();
        let queued = out.find("queued").unwrap();
        assert!(running < queued, "{out}");
        assert!(
            out.contains("first job") && out.contains("second job"),
            "{out}"
        );
        assert!(out.contains("running 1/3 · queued 1"), "{out}");
    }

    #[test]
    fn cancel_running_job_starts_the_queued_one() {
        let mut t = busy();
        press(&mut t.app, KeyCode::Char('c'));
        assert_eq!(t.runner.cancelled().len(), 1);
        t.app.update();
        assert_eq!(t.runner.started(), 2);
        assert_eq!(t.runner.request(1).prompt, "second job");
        let out = render_to_string(&t.app, 110, 10);
        assert!(out.contains("cancelled"), "{out}");
    }

    #[test]
    fn priority_keys_change_queued_job() {
        let mut t = busy();
        press(&mut t.app, KeyCode::Down);
        press(&mut t.app, KeyCode::Char('+'));
        press(&mut t.app, KeyCode::Char('+'));
        let rows = t.app.queue_rows();
        assert_eq!(rows[1].priority, 2);
        assert!(render_to_string(&t.app, 110, 10).contains("+2"));
        press(&mut t.app, KeyCode::Up);
        press(&mut t.app, KeyCode::Char('+'));
        assert!(t.app.error.as_deref().unwrap().contains("no longer queued"));
    }

    #[test]
    fn retry_finished_job_and_view_it() {
        let mut t = busy();
        t.runner.emit(0, RunnerMsg::SpawnFailed("missing".into()));
        t.app.update();
        let failed = t.app.queue_rows();
        assert_eq!(failed[0].status, JobStatus::Running, "second job took over");
        press(&mut t.app, KeyCode::Down);
        assert_eq!(t.app.queue_rows()[1].status, JobStatus::Failed);
        press(&mut t.app, KeyCode::Char('r'));
        assert_eq!(t.app.error, None);
        assert_eq!(t.app.stats().queued, 1);

        press(&mut t.app, KeyCode::Enter);
        assert_eq!(t.app.screen, Screen::Dashboard);
        let shown = t.app.current_job().unwrap();
        assert_eq!(
            shown.status,
            JobStatus::Queued,
            "the retried job is selected"
        );
    }

    #[test]
    fn enter_shows_the_job_on_the_dashboard() {
        let mut t = busy();
        t.runner.emit(
            0,
            RunnerMsg::Event(AgentEvent::Text {
                text: "from the first".into(),
                actor: swarmux_domain::Actor::Lead,
            }),
        );
        t.app.update();
        press(&mut t.app, KeyCode::Enter);
        let out = render_to_string(&t.app, 160, 48);
        assert!(out.contains("from the first"), "{out}");
    }
}
