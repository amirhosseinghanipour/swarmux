use std::cell::RefCell;
use std::collections::VecDeque;

use super::*;
use crate::app::Screen as Tab;
use crate::app::tests_support::test_app;
use crate::ui::buffer_to_string;
use swarmux_application::daemon::{GlyphHints, Hello};
use swarmux_domain::Verdict;

/// The service's end of the connection, in memory: what windows send is
/// queued by the test, what the service sends is kept per window.
#[derive(Default)]
struct Hub {
    incoming: RefCell<VecDeque<HubEvent>>,
    sent: RefCell<Vec<(WindowId, Shown)>>,
    answers: RefCell<Vec<(WindowId, Answer)>>,
    closed: RefCell<Vec<WindowId>>,
}

impl WindowHub for Hub {
    fn poll(&self, _: Duration) -> Vec<HubEvent> {
        self.incoming.borrow_mut().drain(..).collect()
    }

    fn send(&self, window: WindowId, shown: Shown) {
        self.sent.borrow_mut().push((window, shown));
    }

    fn answer(&self, window: WindowId, answer: Answer) {
        self.answers.borrow_mut().push((window, answer));
        self.closed.borrow_mut().push(window);
    }

    fn close(&self, window: WindowId) {
        self.closed.borrow_mut().push(window);
    }
}

impl Hub {
    fn push(&self, event: HubEvent) {
        self.incoming.borrow_mut().push_back(event);
    }

    fn key(&self, window: WindowId, c: char) {
        self.push(HubEvent::Input(
            window,
            WindowInput::Key(KeyInput::plain(KeyName::Char(c))),
        ));
    }

    /// What `window` holds after everything it was sent, as text.
    fn screen(&self, window: WindowId) -> String {
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        for (w, shown) in self.sent.borrow().iter() {
            if let (true, Shown::Frame(f)) = (*w == window, shown) {
                apply(&mut buf, f);
            }
        }
        buffer_to_string(&buf)
    }

    fn sent_to(&self, window: WindowId) -> Vec<Shown> {
        self.sent
            .borrow()
            .iter()
            .filter(|(w, _)| *w == window)
            .map(|(_, s)| s.clone())
            .collect()
    }
}

fn hello(version: &str) -> Hello {
    Hello {
        version: version.into(),
        protocol: PROTOCOL,
        cols: 120,
        rows: 30,
        glyphs: GlyphHints::default(),
        splash: false,
        cwd: Some("/elsewhere".into()),
    }
}

fn info() -> ServiceInfo {
    ServiceInfo {
        pid: 4242,
        version: "0.1.0".into(),
        started_at: 1,
    }
}

const A: WindowId = WindowId(1);
const B: WindowId = WindowId(2);

#[test]
fn frames_rebuild_the_screen_exactly_and_send_only_what_changed() {
    let mut t = test_app();
    let first = t.app.render_buffer(100, 30);
    let full = frame_of(None, &first);
    assert!(full.full);
    let mut held = Buffer::empty(Rect::new(0, 0, 1, 1));
    apply(&mut held, &full);
    assert_eq!(held, first);

    t.app.screen = Tab::Queue;
    let second = t.app.render_buffer(100, 30);
    let diff = frame_of(Some(&first), &second);
    assert!(!diff.full);
    let cells: usize = diff.runs.iter().map(|r| r.cells.len()).sum();
    assert!(cells > 0 && cells < 100 * 30, "{cells} cells");
    apply(&mut held, &diff);
    assert_eq!(held, second);
    // Nothing changed, nothing sent.
    assert!(frame_of(Some(&second), &second).runs.is_empty());
}

#[test]
fn keys_survive_the_trip() {
    for code in [
        KeyCode::Char('q'),
        KeyCode::Enter,
        KeyCode::BackTab,
        KeyCode::F(3),
        KeyCode::PageDown,
    ] {
        let k = KeyEvent::new(code, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        assert_eq!(key_event(key_input(k).unwrap()), k);
    }
    assert_eq!(key_input(KeyEvent::from(KeyCode::Null)), None);
}

#[test]
fn colours_survive_the_trip() {
    for c in [
        Color::Reset,
        Color::Black,
        Color::LightCyan,
        Color::White,
        Color::Indexed(200),
        Color::Rgb(1, 2, 3),
    ] {
        assert_eq!(color(colour(c)), c);
    }
}

/// Acceptance: two windows see one app; an approval answered in one is
/// gone from the other.
#[test]
fn an_approval_answered_in_one_window_is_gone_from_the_other() {
    let mut t = test_app();
    let hub = Hub::default();
    let mut svc = Service::new(&mut t.app, info());
    let id = t.inbox.push(None, "Bash", "make deploy");
    hub.push(HubEvent::Hello(A, hello("0.1.0")));
    hub.push(HubEvent::Hello(B, hello("0.1.0")));
    assert_eq!(svc.step(&mut t.app, &hub), None);
    assert_eq!(svc.windows(), 2);
    for w in [A, B] {
        let Some(Shown::Welcome(welcome)) = hub.sent_to(w).first().cloned() else {
            panic!("{:?}", hub.sent_to(w))
        };
        assert!(welcome.attached);
        assert_eq!(welcome.pid, 4242);
        let screen = hub.screen(w);
        assert!(screen.contains("Bash make deploy"), "{screen}");
    }
    hub.key(A, 'y');
    svc.step(&mut t.app, &hub);
    assert_eq!(t.inbox.answer_to(id).unwrap().verdict, Verdict::Allow);
    let other = hub.screen(B);
    assert!(!other.contains("make deploy"), "{other}");
}

#[test]
fn q_lets_one_window_go_and_the_work_goes_on() {
    let mut t = test_app();
    t.with_session("demo");
    t.send("a long job");
    let hub = Hub::default();
    let mut svc = Service::new(&mut t.app, info());
    t.app.screen = Tab::Queue;
    hub.push(HubEvent::Hello(A, hello("0.1.0")));
    hub.push(HubEvent::Hello(B, hello("0.1.0")));
    svc.step(&mut t.app, &hub);
    let screen = hub.screen(A);
    assert!(screen.contains("q detach"), "{screen}");
    hub.key(A, 'q');
    svc.step(&mut t.app, &hub);
    assert_eq!(svc.windows(), 1);
    assert!(hub.closed.borrow().contains(&A));
    let bye = hub.sent_to(A).into_iter().rev().find_map(|s| match s {
        Shown::Bye(text) => Some(text),
        _ => None,
    });
    assert_eq!(
        bye.as_deref(),
        Some("1 job keeps running in the background · `swarmux daemon stop` to stop them")
    );
    assert!(!t.app.quit);
    assert_eq!(t.app.engine().busy().0, 1, "the job still runs");
    // B goes on being shown the app.
    let before = hub.sent_to(B).len();
    t.app.screen = Tab::Ledger;
    svc.step(&mut t.app, &hub);
    assert!(hub.sent_to(B).len() > before);
}

#[test]
fn a_window_of_another_version_is_refused() {
    let mut t = test_app();
    let hub = Hub::default();
    let mut svc = Service::new(&mut t.app, info());
    hub.push(HubEvent::Hello(A, hello("9.9.9")));
    svc.step(&mut t.app, &hub);
    assert_eq!(svc.windows(), 0);
    let Some(Shown::Welcome(w)) = hub.sent_to(A).first().cloned() else {
        panic!()
    };
    assert!(!w.attached);
    assert_eq!(w.version, "0.1.0");
    let skew = swarmux_application::daemon::skew("9.9.9", &w).unwrap();
    assert!(skew.can_restart, "nothing runs");
    assert!(
        skew.message
            .starts_with("The background service runs swarmux 0.1.0, this is swarmux 9.9.9."),
        "{}",
        skew.message
    );
    assert!(hub.closed.borrow().contains(&A));
}

#[test]
fn status_and_stop_are_answered() {
    let mut t = test_app();
    t.with_session("demo");
    t.send("work");
    let hub = Hub::default();
    let mut svc = Service::new(&mut t.app, info());
    hub.push(HubEvent::Hello(A, hello("0.1.0")));
    hub.push(HubEvent::Asked(B, Ask::Status));
    svc.step(&mut t.app, &hub);
    let (_, Answer::Status(s)) = hub.answers.borrow()[0].clone() else {
        panic!()
    };
    assert_eq!((s.pid, s.running, s.windows), (4242, 1, 1));

    // Stop and wait: nothing new starts and the service goes on until the
    // running job has finished.
    hub.push(HubEvent::Asked(WindowId(3), Ask::Stop { wait: true }));
    assert_eq!(svc.step(&mut t.app, &hub), None);
    assert!(t.app.engine().jobs.draining());
    let job = s_job(&t.app);
    t.app.engine_mut().jobs.cancel(job).unwrap();
    assert_eq!(svc.step(&mut t.app, &hub), Some(true));
    assert!(matches!(hub.sent_to(A).last(), Some(Shown::Bye(_))));

    // Stop now: the service ends at once.
    let mut t = test_app();
    let hub = Hub::default();
    let mut svc = Service::new(&mut t.app, info());
    hub.push(HubEvent::Asked(A, Ask::Stop { wait: false }));
    assert_eq!(svc.step(&mut t.app, &hub), Some(false));
}

/// The one running job.
fn s_job(app: &App) -> swarmux_domain::JobId {
    app.queue_rows()[0].job_id
}

#[test]
fn nobody_attached_counts_as_away_and_a_window_brings_the_user_back() {
    let mut t = test_app();
    let hub = Hub::default();
    let mut svc = Service::new(&mut t.app, info());
    svc.step(&mut t.app, &hub);
    assert!(!svc.focused);
    hub.push(HubEvent::Hello(A, hello("0.1.0")));
    svc.step(&mut t.app, &hub);
    assert!(svc.focused);
    hub.push(HubEvent::Input(A, WindowInput::Focus(false)));
    svc.step(&mut t.app, &hub);
    assert!(!svc.focused);
    hub.push(HubEvent::Left(A));
    svc.step(&mut t.app, &hub);
    assert_eq!(svc.windows(), 0);
}

#[test]
fn a_resized_window_is_sent_its_screen_whole() {
    let mut t = test_app();
    let hub = Hub::default();
    let mut svc = Service::new(&mut t.app, info());
    hub.push(HubEvent::Hello(A, hello("0.1.0")));
    svc.step(&mut t.app, &hub);
    hub.push(HubEvent::Input(
        A,
        WindowInput::Resize { cols: 90, rows: 20 },
    ));
    svc.step(&mut t.app, &hub);
    let Some(Shown::Frame(f)) = hub.sent_to(A).last().cloned() else {
        panic!()
    };
    assert!(f.full);
    assert_eq!((f.cols, f.rows), (90, 20));
}
