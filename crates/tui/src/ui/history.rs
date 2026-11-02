use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Cell, Paragraph, Row, Table, TableState, Wrap},
};
use swarmux_application::JobDetail;
use swarmux_domain::{ReviewState, ReviewVerdict, iso_minute};

use super::{GREEN, MUTED, ORANGE, RED, short, status_span};
use crate::app::App;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let [table_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(area);
    render_table(frame, table_area, app);
    render_detail(frame, detail_area, app.history.detail.as_ref());
}

fn panel(title: String) -> Block<'static> {
    Block::bordered()
        .border_style(Style::new().fg(MUTED))
        .title(title)
}

fn render_table(frame: &mut Frame, area: Rect, app: &App) {
    let h = &app.history;
    let title = if h.query.is_empty() {
        " history (UTC) ".to_string()
    } else {
        format!(" history · \"{}\" · {} found ", h.query, h.rows.len())
    };
    if h.rows.is_empty() {
        let hint = if h.query.is_empty() {
            "no jobs yet"
        } else {
            "nothing matches: press / to change the search"
        };
        frame.render_widget(Paragraph::new(hint).fg(MUTED).block(panel(title)), area);
        return;
    }
    let rows: Vec<Row> = h
        .rows
        .iter()
        .map(|r| {
            Row::new(vec![
                Cell::from(iso_minute(r.created_at)),
                Cell::from(r.session_name.clone()),
                Cell::from(status_span(r.status)),
                Cell::from(format!("${:.4}", r.cost_usd)),
                Cell::from(format!("{}/{}", short(r.tokens_in), short(r.tokens_out))),
                Cell::from(r.preview(80)),
            ])
        })
        .collect();
    let widths = [
        Constraint::Length(16),
        Constraint::Length(14),
        Constraint::Length(11),
        Constraint::Length(8),
        Constraint::Length(11),
        Constraint::Fill(1),
    ];
    let header = Row::new(["time", "session", "status", "cost", "tokens", "prompt"]).fg(MUTED);
    let table = Table::new(rows, widths)
        .header(header)
        .block(panel(title))
        .highlight_symbol("› ")
        .row_highlight_style(Style::new().bold());
    let mut state = TableState::default().with_selected(Some(h.selected));
    frame.render_stateful_widget(table, area, &mut state);
}

fn render_detail(frame: &mut Frame, area: Rect, detail: Option<&JobDetail>) {
    let Some(d) = detail else {
        frame.render_widget(panel(" job ".into()), area);
        return;
    };
    let s = &d.summary;
    let heading = |text: &str| Line::styled(text.to_string(), Style::new().fg(ORANGE));
    let duration = s
        .duration_ms
        .map_or_else(String::new, |ms| format!(" · {:.1}s", ms as f64 / 1000.0));
    let mut lines = vec![
        Line::from(vec![
            Span::raw(format!("{} · {} · ", s.session_name, s.model)),
            status_span(s.status),
        ]),
        Line::styled(
            format!(
                "{} UTC{duration} · ${:.4} · in {} / out {}",
                iso_minute(s.created_at),
                s.cost_usd,
                short(s.tokens_in),
                short(s.tokens_out)
            ),
            Style::new().fg(MUTED),
        ),
        Line::raw(""),
        heading("prompt"),
    ];
    lines.extend(s.prompt.lines().map(|l| Line::raw(l.to_string())));
    if let Some(text) = d.result.as_ref().map(|r| r.text.as_str())
        && !text.is_empty()
    {
        lines.push(Line::raw(""));
        lines.push(heading("result"));
        lines.extend(text.lines().map(|l| Line::raw(l.to_string())));
    }
    if let Some(review) = &d.review {
        lines.push(Line::raw(""));
        lines.push(heading("review"));
        lines.extend(review_lines(review));
    }
    if let Some(e) = &s.error {
        lines.push(Line::raw(""));
        lines.push(Line::styled("error", Style::new().fg(RED)));
        lines.extend(e.lines().map(|l| Line::raw(l.to_string())));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(panel(format!(" job #{} ", s.job_id))),
        area,
    );
}

/// The review's outcome, its concerns and what it cost.
fn review_lines(review: &ReviewState) -> Vec<Line<'static>> {
    let cost = |c: f64| Span::styled(format!(" · ${c:.4}"), Style::new().fg(MUTED));
    match review {
        ReviewState::Requested => vec![Line::styled("requested", Style::new().fg(MUTED))],
        ReviewState::Running { job } => vec![Line::styled(
            format!("running as job #{job}"),
            Style::new().fg(MUTED),
        )],
        ReviewState::Done {
            review,
            job,
            cost_usd,
        } => {
            let (text, color) = match review.verdict {
                ReviewVerdict::Pass => ("passed", GREEN),
                ReviewVerdict::Concerns => ("concerns", RED),
            };
            let mut lines = vec![Line::from(vec![
                Span::styled(text, Style::new().fg(color)),
                Span::raw(format!(
                    " · {} diffs checked · job #{job}",
                    review.diffs_checked
                )),
                cost(*cost_usd),
            ])];
            lines.extend(
                review
                    .concerns
                    .iter()
                    .map(|c| Line::styled(format!("- {c}"), Style::new().fg(RED))),
            );
            lines
        }
        ReviewState::Failed {
            error, cost_usd, ..
        } => vec![Line::from(vec![
            Span::styled(format!("review failed: {error}"), Style::new().fg(RED)),
            cost(*cost_usd),
        ])],
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use swarmux_application::ports::RunnerMsg;
    use swarmux_domain::{AgentEvent, SessionId};

    use crate::app::tests_support::{TestApp, test_app};
    use crate::app::{App, InputPurpose, Mode, Screen};
    use crate::ui::render_to_string;

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    fn init(id: &str) -> RunnerMsg {
        RunnerMsg::Event(AgentEvent::Init {
            session_id: id.into(),
            model: "claude-sonnet".into(),
            cwd: "/work".into(),
            permission_mode: "default".into(),
        })
    }

    fn result(text: &str) -> RunnerMsg {
        RunnerMsg::Event(AgentEvent::Result {
            is_error: false,
            subtype: "success".into(),
            cost_usd: 0.25,
            duration_ms: 2500,
            num_turns: 1,
            tokens_in: 1200,
            tokens_out: 300,
            cache_read: 0,
            context_window: None,
            subagents_spawned: 0,
            text: text.into(),
        })
    }

    /// Two finished jobs in session "demo", on the history screen.
    fn with_history() -> TestApp {
        let mut t = test_app();
        t.with_session("demo");
        t.send("Fix the parser");
        let sid = t.runner.request(0).session_id.to_string();
        t.runner.emit(0, init(&sid));
        t.runner.emit(0, result("parser fixed"));
        t.app.update();
        t.clock.advance(60_000);
        t.send("write the docs");
        t.runner.emit(1, RunnerMsg::SpawnFailed("no claude".into()));
        t.app.update();
        press(&mut t.app, KeyCode::Char('4'));
        t
    }

    #[test]
    fn table_and_detail() {
        let t = with_history();
        let out = render_to_string(&t.app, 140, 16);
        assert!(out.contains("history (UTC)"), "{out}");
        assert!(out.contains("1970-01-01 00:01"), "{out}");
        let docs = out.find("write the docs").unwrap();
        let parser = out.find("Fix the parser").unwrap();
        assert!(docs < parser, "newest first: {out}");
        assert!(out.contains("1.2k/300"), "{out}");
        assert!(out.contains("job #2"), "{out}");
        assert!(out.contains("spawn failed: no claude"), "{out}");
        assert!(out.contains("/ search"), "{out}");
    }

    #[test]
    fn detail_follows_selection_and_shows_result() {
        let mut t = with_history();
        press(&mut t.app, KeyCode::Down);
        let out = render_to_string(&t.app, 140, 16);
        assert!(out.contains("job #1"), "{out}");
        assert!(out.contains("parser fixed"), "{out}");
        assert!(out.contains("2.5s · $0.2500"), "{out}");
    }

    #[test]
    fn detail_shows_the_review() {
        use swarmux_application::ports::JobRepository;
        use swarmux_domain::{JobId, Review, ReviewState, ReviewVerdict};
        let mut t = with_history();
        let review = ReviewState::Done {
            review: Review {
                verdict: ReviewVerdict::Concerns,
                concerns: vec!["no test for empty input".into()],
                diffs_checked: 2,
            },
            job: JobId(9),
            cost_usd: 0.0311,
        };
        t.store.set_review(JobId(1), &review).unwrap();
        press(&mut t.app, KeyCode::Char('4'));
        press(&mut t.app, KeyCode::Down);
        let out = render_to_string(&t.app, 160, 30);
        assert!(
            out.contains("concerns · 2 diffs checked · job #9 · $0.0311"),
            "{out}"
        );
        assert!(out.contains("- no test for empty input"), "{out}");
    }

    #[test]
    fn search_filters_as_you_type_and_esc_clears() {
        let mut t = with_history();
        press(&mut t.app, KeyCode::Char('/'));
        assert!(matches!(t.app.mode, Mode::Input(InputPurpose::Search, _)));
        type_str(&mut t.app, "PARSER");
        assert_eq!(t.app.history.rows.len(), 1);
        assert!(!t.app.quit, "q-like keys type into the search");
        let out = render_to_string(&t.app, 140, 16);
        assert!(out.contains("search: PARSER"), "{out}");
        press(&mut t.app, KeyCode::Enter);
        assert_eq!(t.app.mode, Mode::Normal);
        assert!(render_to_string(&t.app, 140, 16).contains("1 found"));

        press(&mut t.app, KeyCode::Char('/'));
        press(&mut t.app, KeyCode::Esc);
        assert_eq!(t.app.history.rows.len(), 2);
        assert!(t.app.history.query.is_empty());
    }

    #[test]
    fn rerun_queues_the_prompt_again() {
        let mut t = with_history();
        press(&mut t.app, KeyCode::Char('r'));
        assert_eq!(t.app.error, None);
        assert_eq!(t.runner.started(), 3);
        assert_eq!(t.runner.request(2).prompt, "write the docs");
        assert_eq!(t.app.history.rows.len(), 3);
    }

    #[test]
    fn fork_creates_a_new_session_once_claude_reports_it() {
        let mut t = with_history();
        press(&mut t.app, KeyCode::Down);
        press(&mut t.app, KeyCode::Char('f'));
        let req = t.runner.request(2);
        assert!(req.fork && req.resume);
        let new_id = SessionId::random();
        t.runner.emit(2, init(&new_id.to_string()));
        t.app.update();
        let names: Vec<&str> = t.app.sessions.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"demo fork"), "{names:?}");
        assert_eq!(t.app.selected_session().unwrap().name, "demo");
    }

    #[test]
    fn enter_replays_a_job_from_an_earlier_run() {
        let t = with_history();
        // A new app over the same store: its jobs are history, not live.
        let mut t2 = crate::app::tests_support::test_app_with_store(t.store.clone());
        press(&mut t2.app, KeyCode::Char('4'));
        press(&mut t2.app, KeyCode::Down);
        press(&mut t2.app, KeyCode::Enter);
        assert_eq!(t2.app.screen, Screen::Dashboard);
        let out = render_to_string(&t2.app, 160, 48);
        assert!(out.contains("done"), "{out}");
        assert!(out.contains("$0.2500"), "{out}");
    }
}
