//! I1: messages to a running agent, through a real app: the line opens on
//! the stream, nothing is sent without Enter, a queued message is delivered
//! when the turn ends and confirmed when the agent's next turn starts,
//! steering asks twice, and the keys are the keymap's.

use crossterm::event::KeyCode;
use swarmux_application::ports::RunnerMsg;
use swarmux_domain::intervene::{Mode as Via, State};
use swarmux_domain::{AgentEvent, JobId};

use crate::app::tests_support::{TestApp, test_app};
use crate::app::{InputPurpose, Mode};

fn chars(t: &mut TestApp, text: &str) {
    t.keys(text.chars().map(KeyCode::Char));
}

fn init() -> RunnerMsg {
    RunnerMsg::Event(AgentEvent::Init {
        session_id: "s".into(),
        model: "sonnet".into(),
        cwd: "/work".into(),
        permission_mode: "default".into(),
    })
}

/// One session, one job running and reported, on the Work screen.
fn running() -> (TestApp, JobId) {
    let mut t = test_app();
    t.with_session("alpha");
    t.send("first");
    t.app.update();
    t.runner.emit(0, init());
    t.app.update();
    let job = t.app.current_job().unwrap().job_id;
    chars(&mut t, "gw");
    (t, job)
}

fn finish(t: &mut TestApp, i: usize) {
    end(t, i, 0);
}

fn end(t: &mut TestApp, i: usize, code: i32) {
    t.runner.emit(
        i,
        RunnerMsg::Event(AgentEvent::Result {
            is_error: code != 0,
            subtype: "success".into(),
            cost_usd: 0.1,
            duration_ms: 900,
            num_turns: 1,
            tokens_in: 1,
            tokens_out: 1,
            cache_read: 0,
            cache_creation: None,
            context_window: None,
            subagents_spawned: 0,
            text: "done".into(),
        }),
    );
    t.runner.emit(
        i,
        RunnerMsg::Exited {
            code: Some(code),
            stderr_tail: String::new(),
        },
    );
    t.app.update();
}

/// The index of the started job whose prompt says `needle`.
fn started_with(t: &TestApp, needle: &str) -> Option<usize> {
    (0..t.runner.started()).find(|i| t.runner.request(*i).prompt.contains(needle))
}

fn in_message_line(t: &TestApp) -> bool {
    matches!(t.app.mode, Mode::Input(InputPurpose::WorkMessage, _))
}

#[test]
fn m_opens_the_line_on_the_stream_and_esc_sends_nothing() {
    let (mut t, job) = running();
    chars(&mut t, "m");
    assert!(in_message_line(&t));
    assert_eq!(t.app.work_screen.tab, crate::work::Tab::Stream);
    let out = t.app.render_text(160, 40);
    assert!(out.contains("MESSAGE THE AGENT"), "{out}");
    assert!(
        out.contains("nothing is sent before you press enter"),
        "{out}"
    );
    chars(&mut t, "use staging");
    t.keys([KeyCode::Esc]);
    assert_eq!(t.app.mode, Mode::Normal);
    assert_eq!(t.app.notice.as_deref(), Some("not sent"));
    assert_eq!(t.app.engine.interventions.for_job(job).count(), 0);
    assert_eq!(t.runner.started(), 1);
}

#[test]
fn enter_queues_and_the_conversation_shows_it_pending() {
    let (mut t, job) = running();
    chars(&mut t, "m");
    chars(&mut t, "use the staging bucket");
    t.keys([KeyCode::Enter]);
    assert_eq!(t.app.mode, Mode::Normal);
    let notice = t.app.notice.clone().unwrap();
    assert!(notice.contains("queued"), "{notice}");
    let r = t.app.message_receipts(job);
    assert_eq!(r.len(), 1);
    assert_eq!((r[0].mode, &r[0].state), (Via::Queue, &State::Queued));
    let out = t.app.render_text(160, 40);
    for needle in [
        "YOUR MESSAGES",
        "use the staging bucket",
        "queued · delivers when the agent's turn ends",
        "» you",
        "1 message queued",
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
    // Nothing was stopped or sent.
    assert!(t.runner.cancelled().is_empty());
    assert_eq!(t.runner.started(), 1);
}

#[test]
fn a_message_sent_mid_turn_reaches_the_agent_when_the_turn_ends_and_is_confirmed_when_it_starts() {
    let (mut t, job) = running();
    chars(&mut t, "m");
    chars(&mut t, "also cover the empty case");
    t.keys([KeyCode::Enter]);
    // The turn is still going: the message waits.
    t.app.update();
    assert_eq!(started_with(&t, "also cover the empty case"), None);

    finish(&mut t, 0);
    t.app.update();
    let at = started_with(&t, "also cover the empty case")
        .expect("the session continued with the message");
    assert!(
        t.runner
            .request(at)
            .prompt
            .contains("while you were working")
    );
    assert!(matches!(
        t.app.message_receipts(job)[0].state,
        State::Handed { .. }
    ));
    let out = t.app.render_text(160, 40);
    assert!(
        !out.contains("delivered ·"),
        "not before the turn starts\n{out}"
    );

    t.runner.emit(at, init());
    t.app.update();
    t.app.update();
    let state = t.app.message_receipts(job)[0].state.clone();
    assert!(matches!(state, State::Delivered { .. }), "{state:?}");
}

#[test]
fn steering_asks_twice_then_stops_the_turn() {
    let (mut t, job) = running();
    chars(&mut t, "m");
    t.keys([KeyCode::Tab]);
    chars(&mut t, "use orders_v2");
    let out = t.app.render_text(160, 40);
    assert!(out.contains("steer"), "{out}");
    assert!(out.contains("cut short"), "{out}");
    t.keys([KeyCode::Enter]);
    // Not yet: the line stays open and asks for the confirmation.
    assert!(in_message_line(&t));
    assert!(t.runner.cancelled().is_empty());
    assert!(
        t.app
            .notice
            .clone()
            .unwrap()
            .contains("enter again to confirm")
    );
    let out = t.app.render_text(160, 40);
    assert!(out.contains("enter again stops the agent's turn"), "{out}");

    t.keys([KeyCode::Enter]);
    assert_eq!(t.runner.cancelled(), vec![job]);
    let r = t.app.message_receipts(job);
    assert!(
        matches!(r[0].state, State::Handed { .. }),
        "{:?}",
        r[0].state
    );
    assert!(started_with(&t, "use orders_v2").is_some());
}

#[test]
fn tab_switches_the_mode_back_and_forth_and_drops_a_pending_confirmation() {
    let (mut t, job) = running();
    chars(&mut t, "m");
    chars(&mut t, "x");
    t.keys([KeyCode::Tab]);
    t.keys([KeyCode::Enter]);
    assert!(t.app.work_screen.message.as_ref().unwrap().armed);
    t.keys([KeyCode::Tab]);
    let c = t.app.work_screen.message.clone().unwrap();
    assert_eq!((c.mode, c.armed), (Via::Queue, false));
    // The text stayed in the line.
    t.keys([KeyCode::Enter]);
    assert_eq!(t.app.engine.interventions.waiting(job), 1);
}

#[test]
fn an_empty_message_keeps_the_line_open_and_says_so() {
    let (mut t, job) = running();
    chars(&mut t, "m");
    t.keys([KeyCode::Enter]);
    assert!(t.app.notice.clone().unwrap().contains("write a message"));
    assert!(t.app.work_screen.message.is_some());
    assert!(in_message_line(&t));
    assert_eq!(t.app.engine.interventions.for_job(job).count(), 0);
}

#[test]
fn a_held_call_refuses_steering_and_the_line_stays_to_switch_to_queue() {
    let (mut t, job) = running();
    t.inbox.push(Some(job), "Bash", "make deploy");
    t.app.update();
    chars(&mut t, "m");
    t.keys([KeyCode::Tab]);
    let out = t.app.render_text(160, 40);
    assert!(out.contains("a call is held for you"), "{out}");
    chars(&mut t, "wait");
    t.keys([KeyCode::Enter]);
    t.keys([KeyCode::Enter]);
    assert!(
        t.app
            .notice
            .clone()
            .unwrap()
            .contains("a call is held for you")
    );
    assert!(t.runner.cancelled().is_empty());
    assert_eq!(t.app.engine.guard.pending_count(), 1, "still held");
    // Switch to queue: the same text goes.
    t.keys([KeyCode::Tab]);
    t.keys([KeyCode::Enter]);
    assert_eq!(t.app.engine.interventions.waiting(job), 1);
    assert_eq!(
        t.app.engine.guard.pending_count(),
        1,
        "a message approves nothing"
    );
}

#[test]
fn a_job_that_ended_says_why_it_cannot_be_messaged() {
    let (mut t, _) = running();
    // A failed job waits on the DECIDE list: the outcome is still there.
    end(&mut t, 0, 1);
    t.app.update();
    chars(&mut t, "m");
    assert_eq!(t.app.mode, Mode::Normal);
    let notice = t.app.notice.clone().unwrap();
    assert!(notice.starts_with("message: "), "{notice}");
    assert!(notice.contains("send it back with a note"), "{notice}");
}

#[test]
fn the_actions_panel_offers_message_with_its_key_while_the_job_runs() {
    let (t, _) = running();
    let out = t.app.render_text(160, 40);
    assert!(
        out.lines()
            .any(|l| l.contains(" m ") && l.contains("message")),
        "{out}"
    );
}

#[test]
fn the_message_keys_are_rebindable_under_keys_work() {
    let (mut t, _) = running();
    t.app = t.app.with_key_config(&crate::keys::config_of(&[
        ("work", "work.message", &["i"]),
        ("work", "work.message_mode", &["I"]),
    ]));
    chars(&mut t, "m");
    assert_eq!(t.app.mode, Mode::Normal, "m is free now");
    chars(&mut t, "i");
    assert!(in_message_line(&t));
    t.keys([KeyCode::Esc]);
    chars(&mut t, "I");
    assert_eq!(t.app.work_screen.mode, Via::Steer);
    let out = t.app.render_text(160, 40);
    assert!(
        out.lines()
            .any(|l| l.contains(" i ") && l.contains("message")),
        "{out}"
    );
}

/// The screen at the two sizes the console is designed for.
const SIZES: [(u16, u16); 2] = [(180, 50), (120, 36)];

/// No line is wider than the screen, and nothing the conversation says is
/// cut off at its heading or its state.
fn fits(out: &str, w: u16) {
    for line in out.lines() {
        assert!(line.chars().count() <= usize::from(w), "{line}");
    }
}

#[test]
fn the_composer_is_drawn_in_each_mode_at_both_sizes() {
    for (w, h) in SIZES {
        let (mut t, _) = running();
        chars(&mut t, "m");
        chars(&mut t, "use the staging bucket");
        let out = t.app.render_text(w, h);
        fits(&out, w);
        for needle in [
            "MESSAGE THE AGENT",
            "mode   queue",
            "delivered when the agent's turn ends; nothing is interrupted",
            "message use the staging bucket",
            "nothing is sent before you press enter",
        ] {
            assert!(out.contains(needle), "{w}x{h}: {needle}\n{out}");
        }
        // The line at the bottom says the same keys (a long text pushes them
        // under the status on the narrow screen; the panel above has them).
        if w >= 180 {
            assert!(out.contains("enter send · tab mode · esc drop"), "{out}");
        }

        t.keys([KeyCode::Tab]);
        let out = t.app.render_text(w, h);
        fits(&out, w);
        for needle in ["mode   steer", "stops the agent's turn now", "cut short"] {
            assert!(out.contains(needle), "{w}x{h}: {needle}\n{out}");
        }
        t.keys([KeyCode::Enter]);
        let out = t.app.render_text(w, h);
        fits(&out, w);
        assert!(
            out.contains("enter again stops the agent's turn and sends it"),
            "{w}x{h}\n{out}"
        );
    }
}

#[test]
fn a_receipt_is_drawn_in_each_state_at_both_sizes() {
    for (w, h) in SIZES {
        // Queued, then handed over, then delivered.
        let (mut t, job) = running();
        chars(&mut t, "m");
        chars(&mut t, "keep the old column");
        t.keys([KeyCode::Enter]);
        let out = t.app.render_text(w, h);
        fits(&out, w);
        for needle in [
            "YOUR MESSAGES",
            "#1 queue keep the old column",
            "queued · delivers when the agent's turn ends",
            "1 message queued",
        ] {
            assert!(out.contains(needle), "{w}x{h}: {needle}\n{out}");
        }

        finish(&mut t, 0);
        t.app.update();
        let out = t.app.render_text(w, h);
        fits(&out, w);
        // The conversation follows the work: the lead job is now the
        // continuation, and its pane says what carries the message.
        assert!(
            out.contains("not confirmed until its turn starts"),
            "{w}x{h}\n{out}"
        );
        assert!(!out.contains("delivered ·"), "{w}x{h}\n{out}");

        let at = started_with(&t, "keep the old column").unwrap();
        t.runner.emit(at, init());
        t.app.update();
        t.app.update();
        let out = t.app.render_text(w, h);
        fits(&out, w);
        assert!(
            out.contains("delivered · its turn started as job #"),
            "{w}x{h}\n{out}"
        );
        let _ = job;
    }
}

#[test]
fn a_message_that_could_not_be_delivered_is_drawn_with_its_reason() {
    for (w, h) in SIZES {
        let (mut t, job) = running();
        chars(&mut t, "m");
        chars(&mut t, "never mind the tests");
        t.keys([KeyCode::Enter]);
        // The job is killed before its turn ends.
        t.app.engine.jobs.cancel(job).unwrap();
        t.app.update();
        t.app.update();
        let r = t.app.message_receipts(job);
        assert!(
            matches!(r[0].state, State::Undelivered(_)),
            "{:?}",
            r[0].state
        );
        // A job the user stopped leaves the queue, so the conversation is
        // gone with it: the failure is said once, where they are looking.
        let notice = t.app.notice.clone().unwrap();
        assert!(
            notice.contains("not delivered: the job was stopped before its turn ended"),
            "{notice}"
        );
        let out = t.app.render_text(w, h);
        fits(&out, w);
        assert!(
            out.contains("not delivered: the job was stopped"),
            "{w}x{h}\n{out}"
        );
    }
}

#[test]
fn the_configured_default_mode_is_what_the_line_opens_in() {
    let (mut t, _) = running();
    t.app.work_screen.mode = Via::Steer;
    chars(&mut t, "m");
    assert_eq!(t.app.work_screen.message.as_ref().unwrap().mode, Via::Steer);
}
