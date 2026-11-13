//! Guard decision lines for the dashboard, and the approval overlay on every
//! screen.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};
use swarmux_application::{DecisionRecord, Outcome};

use crate::app::App;
use crate::theme::Theme;

/// Height of the approval overlay, borders included.
pub const APPROVAL_HEIGHT: u16 = 6;
/// Below this body height the approval is one line, so the wall stays visible.
pub const COMPACT_BELOW: u16 = 24;

pub(crate) fn decision_line<'a>(d: &'a DecisionRecord, t: &Theme) -> Line<'a> {
    let color = match d.outcome {
        Outcome::Allow | Outcome::Route => t.ok,
        Outcome::Deny | Outcome::Stop => t.error,
        Outcome::Ask => t.lead,
        Outcome::Pass => t.muted,
    };
    let job = d.job_id.map(|j| format!("#{j} ")).unwrap_or_default();
    let subject = shorten(&d.subject, 48);
    if d.outcome == Outcome::Route {
        return Line::from(vec![
            Span::styled("route ", Style::new().fg(color)),
            Span::raw(format!("{} ", shorten(&d.subject, 32))),
            Span::raw(d.rule.as_deref().unwrap_or_default().to_string()),
            Span::styled(
                format!("  {} · {}", d.source.label(), latency(d.latency_us)),
                Style::new().fg(t.muted),
            ),
        ]);
    }
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
            Style::new().fg(t.muted),
        ),
    ])
}

/// The held tool call as one line at the bottom of a small window:
/// `Bash make deploy · #3 · 95 s   y n a  +1`.
pub fn render_approval_bar(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let Some(first) = app.approval_target() else {
        return;
    };
    let r = &first.request;
    let more = match app.engine.guard.pending_count() {
        0 | 1 => String::new(),
        n => format!(" +{}", n - 1),
    };
    let job = r.job_id.map_or(String::new(), |id| format!(" · #{id}"));
    let tail = format!("{job} · {} s  ", first.secs_left);
    let keys = if App::is_irreversible(&first) {
        format!("y n{more}")
    } else {
        format!("y n a{more}")
    };
    let what = if r.subject.is_empty() {
        shorten(&r.input, 200)
    } else {
        shorten(&r.subject, 200)
    };
    let fixed =
        1 + r.tool_name.chars().count() + 1 + tail.chars().count() + keys.chars().count() + 1;
    let room = (area.width as usize).saturating_sub(fixed);
    let what: String = if what.chars().count() > room {
        let cut: String = what.chars().take(room.saturating_sub(1)).collect();
        format!("{cut}…")
    } else {
        what
    };
    let bold = |c| Style::new().fg(c).add_modifier(Modifier::BOLD);
    let line = Line::from(vec![
        Span::styled(format!(" {} ", r.tool_name), bold(t.lead)),
        Span::raw(what),
        Span::styled(tail, Style::new().fg(t.muted)),
        Span::styled(keys, bold(t.ok)),
        Span::raw(" "),
    ]);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(line).style(Style::new().bg(t.panel_paint())),
        area,
    );
}

/// The oldest held tool call, its job and how long until it is denied.
pub fn render_approval(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let approvals = app.engine.guard.approvals();
    let Some(first) = app.approval_target() else {
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
    let item = app.mission.alarms().find_request(r.request_id);
    let irreversible = App::is_irreversible(&first);
    let tag = item
        .and_then(|i| i.risk)
        .map(|k| {
            format!(
                "{} {} · score {:.2}",
                k.reversibility.as_str(),
                k.tier.as_str(),
                k.score
            )
        })
        .unwrap_or_default();
    let batch = item.map_or(0, |i| i.batch);
    let mut keys = vec![
        Span::styled("y", Style::new().fg(t.ok).add_modifier(Modifier::BOLD)),
        Span::raw(if irreversible {
            " confirm (name the target) · "
        } else {
            " approve · "
        }),
        Span::styled("n", Style::new().fg(t.error).add_modifier(Modifier::BOLD)),
        Span::raw(" deny"),
    ];
    if !irreversible {
        keys.extend([
            Span::raw(" · "),
            Span::styled("a", Style::new().fg(t.ok).add_modifier(Modifier::BOLD)),
            Span::raw(" always allow"),
        ]);
        if batch > 1 {
            keys.extend([
                Span::raw(" · "),
                Span::styled("A", Style::new().fg(t.ok).add_modifier(Modifier::BOLD)),
                Span::raw(format!(" all {batch}")),
            ]);
        }
    }
    keys.push(Span::styled(more, Style::new().fg(t.lead)));
    let lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{} ", r.tool_name),
                Style::new().fg(t.lead).add_modifier(Modifier::BOLD),
            ),
            Span::raw(what),
        ]),
        Line::styled(format!("{job} · in {}", r.cwd), Style::new().fg(t.muted)),
        Line::styled(
            format!("denied in {} s unless you answer · {tag}", first.secs_left),
            Style::new().fg(t.muted),
        ),
        Line::from(keys),
    ];
    let block = super::rounded(t)
        .border_style(Style::new().fg(t.lead))
        .title(Span::styled(
            " approval needed ",
            Style::new().fg(t.lead).add_modifier(Modifier::BOLD),
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
        for screen in crate::screens::all() {
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
        assert_eq!(t.app.engine.guard.pending_count(), 0);
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
        let kept = t.rules.rules();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].text, "Bash(cargo fmt)");
        assert_eq!(kept[0].scope.as_str(), "repo");
        assert!(
            kept[0].expires_at.is_some(),
            "an always-allow expires by default"
        );
        let out = render_to_string(&t.app, 120, 30);
        assert!(
            out.contains("always allowing: Bash(cargo fmt) for repo"),
            "{out}"
        );
        assert!(out.contains("until "), "{out}");

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
        let out = render_to_string(&t.app, 160, 48);
        assert!(out.contains("GUARD · fork layer · 1 decision "), "{out}");
        assert!(out.contains("allow 100% of 1"), "{out}");
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
        let out = render_to_string(&t.app, 160, 48);
        assert!(out.contains("stop 1 · split → you 0"), "{out}");
        assert!(
            out.contains("loop guard: Bash failed the same way twice"),
            "{out}"
        );
        assert_eq!(t.decisions.rows().len(), 1);
    }
}
