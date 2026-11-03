use std::cell::RefCell;
use std::rc::Rc;

use swarmux_domain::Likelihood;

use super::*;

#[derive(Default)]
struct Log(Rc<RefCell<Vec<Notification>>>);

impl Notifier for Log {
    fn notify(&self, n: &Notification) -> AppResult<()> {
        self.0.borrow_mut().push(n.clone());
        Ok(())
    }
}

fn item(key: &str, priority: Priority) -> AttentionItem {
    AttentionItem {
        id: AttentionId::new(key),
        kind: AttentionKind::Approval,
        priority,
        title: format!("title of {key}"),
        detail: String::new(),
        session: None,
        job: None,
        request: None,
        since: 0,
        deadline: None,
        likelihood: Likelihood::Certain,
    }
}

fn utc() -> i32 {
    0
}

fn service(settings: NotifySettings) -> (NotifyService, Rc<RefCell<Vec<Notification>>>) {
    let log = Log::default();
    let seen = log.0.clone();
    (NotifyService::new(Box::new(log), settings, utc), seen)
}

fn at(hh: i64, mm: i64) -> Millis {
    (hh * 60 + mm) * 60_000
}

#[test]
fn unfocused_p2_notifies_once() {
    let (mut s, seen) = service(NotifySettings::default());
    let items = [item("a", Priority::P2)];
    assert!(s.observe(&items, true, at(12, 0)).is_empty());
    s.observe(&items, true, at(12, 1));
    s.observe(&items, true, at(12, 2));
    assert_eq!(seen.borrow().len(), 1);
}

#[test]
fn focused_p2_waits_then_notifies_when_the_user_leaves() {
    let (mut s, seen) = service(NotifySettings::default());
    let items = [item("a", Priority::P2)];
    s.observe(&items, false, at(12, 0));
    assert!(seen.borrow().is_empty());
    s.observe(&items, true, at(12, 1));
    assert_eq!(seen.borrow().len(), 1);
}

#[test]
fn p3_never_notifies() {
    let (mut s, seen) = service(NotifySettings::default());
    s.observe(&[item("a", Priority::P3)], true, at(12, 0));
    assert!(seen.borrow().is_empty());
}

#[test]
fn p1_always_notifies_even_focused_and_quiet() {
    let settings = NotifySettings {
        quiet: QuietHours::parse("22:00-08:00"),
        ..NotifySettings::default()
    };
    let (mut s, seen) = service(settings);
    s.observe(&[item("a", Priority::P1)], false, at(23, 0));
    assert_eq!(seen.borrow().len(), 1);
}

#[test]
fn the_kill_switch_silences_p1_too() {
    let settings = NotifySettings {
        enabled: false,
        ..NotifySettings::default()
    };
    let (mut s, seen) = service(settings);
    s.observe(&[item("a", Priority::P1), item("b", Priority::P2)], true, 0);
    assert!(seen.borrow().is_empty());
}

#[test]
fn quiet_hours_hold_p2_for_good() {
    let settings = NotifySettings {
        quiet: QuietHours::parse("22:00-08:00"),
        ..NotifySettings::default()
    };
    let (mut s, seen) = service(settings);
    let items = [item("a", Priority::P2)];
    s.observe(&items, true, at(23, 0));
    s.observe(&items, true, at(9, 0));
    assert!(seen.borrow().is_empty());
    // A new item after the window notifies.
    s.observe(&[item("b", Priority::P2)], true, at(9, 1));
    assert_eq!(seen.borrow().len(), 1);
}

#[test]
fn a_gone_item_may_notify_again_when_it_returns() {
    let (mut s, seen) = service(NotifySettings::default());
    s.observe(&[item("a", Priority::P2)], true, 0);
    s.observe(&[], true, 1);
    s.observe(&[item("a", Priority::P2)], true, 2);
    assert_eq!(seen.borrow().len(), 2);
}

#[test]
fn a_burst_becomes_one_summary() {
    let (mut s, seen) = service(NotifySettings::default());
    let items: Vec<_> = (0..6)
        .map(|i| item(&format!("k{i}"), Priority::P2))
        .collect();
    s.observe(&items, true, 0);
    assert_eq!(seen.borrow().len(), 1);
    assert_eq!(seen.borrow()[0].title, "swarmux: 6 items need you");
}

#[test]
fn local_offset_shifts_the_quiet_window() {
    // 21:30 UTC is 00:30 at +03:00, inside 22:00-08:00.
    assert_eq!(local_minute(at(21, 30), 180), 30);
    let q = QuietHours::parse("22:00-08:00").unwrap();
    assert!(q.contains(30));
    assert!(!q.contains(12 * 60));
}

#[test]
fn quiet_hours_parse() {
    let q = QuietHours::parse("09:00-17:30").unwrap();
    assert!(q.contains(9 * 60) && q.contains(17 * 60 + 29));
    assert!(!q.contains(17 * 60 + 30));
    for bad in [
        "",
        "22:00",
        "25:00-08:00",
        "9:00-17:00",
        "10:00-10:00",
        "a-b",
    ] {
        assert_eq!(QuietHours::parse(bad), None, "{bad}");
    }
}

#[test]
fn focus_uses_reports_else_idle_time() {
    let mut f = FocusTracker::default();
    f.on_input(1_000);
    assert!(!f.unfocused(1_000 + IDLE_UNFOCUSED_MS - 1));
    assert!(f.unfocused(1_000 + IDLE_UNFOCUSED_MS));
    f.on_focus(true);
    assert!(!f.unfocused(10 * IDLE_UNFOCUSED_MS));
    f.on_focus(false);
    assert!(f.unfocused(0));
}

#[test]
fn bodies_are_one_short_line() {
    let mut i = item("a", Priority::P2);
    i.title = format!("Bash\nrm {}", "x".repeat(300));
    assert!(!one(&i).body.contains('\n'));
    assert!(one(&i).body.chars().count() <= BODY_MAX);
}
