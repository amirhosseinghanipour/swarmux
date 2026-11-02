//! The guard panel on the dashboard and the approval overlay on every screen.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
};
use swarmux_application::{DecisionRecord, Outcome};

use super::{GREEN, MUTED, ORANGE, RED};
use crate::app::App;

/// Height of the guard panel, borders included.
pub const PANEL_HEIGHT: u16 = 8;
/// Height of the approval overlay, borders included.
pub const APPROVAL_HEIGHT: u16 = 6;

/// Counts for this run and the latest decisions.
pub fn render_panel(frame: &mut Frame, area: Rect, app: &App) {
    let s = app.guard.stats();
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("allow {}", s.allowed), Style::new().fg(GREEN)),
        Span::styled(" · ", Style::new().fg(MUTED)),
        Span::styled(format!("deny {}", s.denied), Style::new().fg(RED)),
        Span::styled(" · ", Style::new().fg(MUTED)),
        Span::styled(format!("ask {}", s.asked), Style::new().fg(ORANGE)),
        Span::styled(" · ", Style::new().fg(MUTED)),
        Span::styled(format!("pass {}", s.passed), Style::new().fg(MUTED)),
        Span::styled(" · ", Style::new().fg(MUTED)),
        Span::styled(format!("loop stops {}", s.stopped), Style::new().fg(RED)),
    ])];
    let room = area.height.saturating_sub(3) as usize;
    let recent: Vec<&DecisionRecord> = app.guard.recent().take(room).collect();
    if recent.is_empty() {
        lines.push(Line::styled(
            "no tool calls yet: every call your jobs make is checked here",
            Style::new().fg(MUTED),
        ));
    }
    lines.extend(recent.into_iter().map(decision_line));
    let block = Block::bordered()
        .border_style(Style::new().fg(GREEN))
        .title(Span::styled(" guard ", Style::new().fg(GREEN)));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn decision_line(d: &DecisionRecord) -> Line<'_> {
    let color = match d.outcome {
        Outcome::Allow => GREEN,
        Outcome::Deny | Outcome::Stop => RED,
        Outcome::Ask => ORANGE,
        Outcome::Pass => MUTED,
    };
    let job = d.job_id.map(|j| format!("#{j} ")).unwrap_or_default();
    let subject = shorten(&d.subject, 48);
    Line::from(vec![
        Span::styled(
            format!("{:<5} ", d.outcome.as_str()),
            Style::new().fg(color),
        ),
        Span::raw(format!("{job}{} {subject}", d.tool)),
        Span::styled(
            format!(
                "  {}  {}",
                d.rule.as_deref().unwrap_or("no rule"),
                latency(d.latency_us)
            ),
            Style::new().fg(MUTED),
        ),
    ])
}

/// The oldest held tool call, its job and how long until it is denied.
pub fn render_approval(frame: &mut Frame, area: Rect, app: &App) {
    let approvals = app.guard.approvals();
    let Some(first) = approvals.first() else {
        return;
    };
    let r = &first.request;
    let job = match r.job_id {
        Some(id) => match app.session_of(id) {
            Some(session) => format!("job #{id} · session {session}"),
            None => format!("job #{id}"),
        },
        None => "unknown job".to_string(),
    };
    let what = if r.subject.is_empty() {
        shorten(&r.input, 200)
    } else {
        shorten(&r.subject, 200)
    };
    let more = match approvals.len() - 1 {
        0 => String::new(),
        n => format!("   (+{n} more waiting)"),
    };
    let lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{} ", r.tool_name),
                Style::new().fg(ORANGE).add_modifier(Modifier::BOLD),
            ),
            Span::raw(what),
        ]),
        Line::styled(format!("{job} · in {}", r.cwd), Style::new().fg(MUTED)),
        Line::styled(
            format!("denied in {} s unless you answer", first.secs_left),
            Style::new().fg(MUTED),
        ),
        Line::from(vec![
            Span::styled("y", Style::new().fg(GREEN).add_modifier(Modifier::BOLD)),
            Span::raw(" approve · "),
            Span::styled("n", Style::new().fg(RED).add_modifier(Modifier::BOLD)),
            Span::raw(" deny · "),
            Span::styled("a", Style::new().fg(GREEN).add_modifier(Modifier::BOLD)),
            Span::raw(" always allow"),
            Span::styled(more, Style::new().fg(ORANGE)),
        ]),
    ];
    let block = Block::bordered()
        .border_style(Style::new().fg(ORANGE))
        .title(Span::styled(
            " approval needed ",
            Style::new().fg(ORANGE).add_modifier(Modifier::BOLD),
        ));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// 12 → "12µs", 1500 → "1.5ms", 30_000_000 → "30s".
pub fn latency(us: u64) -> String {
    match us {
        0..1_000 => format!("{us}µs"),
        1_000..1_000_000 => format!("{:.1}ms", us as f64 / 1e3),
        _ => format!("{}s", us / 1_000_000),
    }
}

/// First line of `s`, cut to `max` characters.
fn shorten(s: &str, max: usize) -> String {
    let first = s.lines().next().unwrap_or_default();
    match first.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &first[..i]),
        None => first.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Screen;
    use crate::app::tests_support::test_app;
    use crate::ui::render_to_string;
    use crossterm::event::KeyCode;
    use swarmux_application::ports::RunnerMsg;
    use swarmux_domain::{Actor, AgentEvent, Verdict};

    #[test]
    fn latency_units() {
        assert_eq!(latency(12), "12µs");
        assert_eq!(latency(1_500), "1.5ms");
        assert_eq!(latency(30_000_000), "30s");
    }

    #[test]
    fn approval_overlay_shows_on_every_screen() {
        let mut t = test_app();
        t.with_session("demo");
        t.send("hi");
        let job = t.app.current_job().unwrap().job_id;
        t.inbox.push(Some(job), "Bash", "make deploy");
        t.app.update();
        for screen in Screen::ALL {
            t.app.screen = screen;
            let out = render_to_string(&t.app, 120, 30);
            assert!(out.contains("approval needed"), "{screen:?}\n{out}");
            assert!(out.contains("Bash make deploy"), "{out}");
            assert!(out.contains(&format!("job #{job} · session demo")), "{out}");
            assert!(out.contains("denied in 120 s"), "{out}");
            assert!(out.contains("approvals 1"), "status bar count\n{out}");
        }
        t.clock.advance(20_000);
        assert!(render_to_string(&t.app, 120, 30).contains("denied in 100 s"));
    }

    #[test]
    fn y_approves_and_the_overlay_goes_away() {
        let mut t = test_app();
        let id = t.inbox.push(None, "Bash", "make");
        t.app.update();
        t.keys([KeyCode::Char('y')]);
        assert_eq!(t.inbox.answer_to(id).unwrap().verdict, Verdict::Allow);
        assert_eq!(t.app.guard.pending_count(), 0);
        let out = render_to_string(&t.app, 120, 30);
        assert!(!out.contains("approval needed"), "{out}");
        assert!(!out.contains("approvals"), "{out}");
    }

    #[test]
    fn n_denies_the_oldest_first() {
        let mut t = test_app();
        let first = t.inbox.push(None, "Bash", "one");
        let second = t.inbox.push(None, "Bash", "two");
        t.app.update();
        assert!(render_to_string(&t.app, 120, 30).contains("+1 more waiting"));
        t.keys([KeyCode::Char('n')]);
        assert_eq!(t.inbox.answer_to(first).unwrap().verdict, Verdict::Deny);
        assert_eq!(t.inbox.answer_to(second), None);
    }

    #[test]
    fn a_always_allows_and_says_where_it_went() {
        let mut t = test_app();
        let id = t.inbox.push(None, "Bash", "cargo  fmt");
        t.app.update();
        t.keys([KeyCode::Char('a')]);
        assert_eq!(t.inbox.answer_to(id).unwrap().verdict, Verdict::Allow);
        assert_eq!(t.rules.rules(), ["Bash(cargo fmt)"]);
        let out = render_to_string(&t.app, 120, 30);
        assert!(
            out.contains("always allowing Bash(cargo fmt) (saved"),
            "{out}"
        );

        t.rules.refuse(true);
        t.inbox.push(None, "Bash", "ls -la");
        t.app.update();
        t.keys([KeyCode::Char('a')]);
        assert!(t.app.notice.as_deref().unwrap().contains("not saved"));
    }

    #[test]
    fn keys_type_normally_while_writing() {
        let mut t = test_app();
        t.with_session("demo");
        let id = t.inbox.push(None, "Bash", "make");
        t.app.update();
        t.keys([KeyCode::Char('p'), KeyCode::Char('y')]);
        assert_eq!(t.inbox.answer_to(id), None);
        assert!(matches!(&t.app.mode, crate::app::Mode::Input(_, i) if i.text == "y"));
    }

    #[test]
    fn dashboard_guard_panel_counts_and_lists_decisions() {
        let mut t = test_app();
        t.with_session("demo");
        t.send("hi");
        t.inbox.push(None, "Read", "/w/src/main.rs");
        t.app.update();
        let out = render_to_string(&t.app, 120, 30);
        assert!(out.contains(" guard "), "{out}");
        assert!(out.contains("allow 1 · deny 0 · ask 0 · pass 0"), "{out}");
        assert!(out.contains("Read /w/src/main.rs  Read(**)"), "{out}");
    }

    #[test]
    fn loop_stops_appear_in_the_panel() {
        let mut t = test_app();
        t.with_session("demo");
        t.send("fix");
        for id in ["a", "b"] {
            t.runner.emit(
                0,
                RunnerMsg::Event(AgentEvent::ToolUse {
                    id: id.into(),
                    name: "Bash".into(),
                    summary: "cargo test".into(),
                    actor: Actor::Lead,
                }),
            );
            t.runner.emit(
                0,
                RunnerMsg::Event(AgentEvent::ToolResult {
                    tool_use_id: id.into(),
                    is_error: true,
                    preview: "error: boom".into(),
                    actor: Actor::Lead,
                }),
            );
        }
        t.app.update();
        let out = render_to_string(&t.app, 140, 30);
        assert!(out.contains("loop stops 1"), "{out}");
        assert!(
            out.contains("loop guard: Bash failed the same way twice"),
            "{out}"
        );
        assert_eq!(t.decisions.rows().len(), 1);
    }
}
