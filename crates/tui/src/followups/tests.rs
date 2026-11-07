use std::rc::Rc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use swarmux_application::testing::InMemoryFollowups;
use swarmux_application::{FollowSettings, FollowUpService};
use swarmux_domain::AttentionKind;
use swarmux_domain::followup::{FollowKind, FollowStatus, FollowUp, Source, Urgency};

use crate::app::Screen;
use crate::app::tests_support::{TestApp, test_app};

struct Rig {
    t: TestApp,
    svc: Rc<FollowUpService>,
}

fn rig() -> Rig {
    let mut t = test_app();
    t.with_session("api");
    let mem = Rc::new(InMemoryFollowups::default());
    let svc = Rc::new(FollowUpService::new(
        mem.clone(),
        mem,
        t.clock.clone(),
        FollowSettings::default(),
    ));
    t.app.engine.followups = Some(svc.clone());
    Rig { t, svc }
}

impl Rig {
    fn key(&mut self, c: char) {
        self.t
            .app
            .on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }

    fn add(&self, kind: FollowKind, text: &str) -> i64 {
        self.svc
            .ingest(FollowUp::new(kind, text, Source::default(), 0), false)
            .unwrap()
    }
}

#[test]
fn only_urgent_follow_ups_reach_the_mission_control_inbox() {
    let mut r = rig();
    r.add(FollowKind::SuggestedTask, "tidy the readme");
    let mut risk = FollowUp::new(FollowKind::Risk, "logs a secret", Source::default(), 0);
    risk.urgency = Urgency::High;
    r.svc.ingest(risk, false).unwrap();
    r.t.clock.advance(10_000);
    r.t.app.update();
    r.t.clock.advance(10_000);
    r.t.app.update();
    let items: Vec<_> =
        r.t.app
            .mission
            .items
            .iter()
            .filter(|i| i.kind == AttentionKind::FollowUp)
            .collect();
    assert_eq!(items.len(), 1);
    assert!(items[0].title.contains("logs a secret"));
}

#[test]
fn screen_nine_shows_cards_with_a_reason_to_say_no() {
    let mut r = rig();
    r.add(FollowKind::SuggestedTask, "tidy the readme");
    r.key('9');
    assert_eq!(r.t.app.screen, Screen::Followups);
    r.t.app.refresh_followups();
    let text = r.t.app.render_buffer(100, 30);
    let text = crate::ui::buffer_to_string(&text);
    assert!(text.contains("tidy the readme"), "{text}");
    assert!(text.contains("you might say no because"), "{text}");
}

#[test]
fn one_key_per_card_approves_declines_and_defers() {
    let mut r = rig();
    let a = r.add(FollowKind::SuggestedTask, "first thing to do");
    let b = r.add(FollowKind::Decision, "REST or gRPC for the new api");
    let c = r.add(FollowKind::Risk, "migration may lock the users table");
    r.key('9');
    r.t.app.refresh_followups();
    // Risks come first, then the older ones.
    assert_eq!(r.t.app.followup_screen.cards[0].id, c);
    r.key('d');
    assert_eq!(
        r.svc.get(c).unwrap().unwrap().status,
        FollowStatus::Declined
    );
    r.key('l');
    assert_eq!(
        r.svc.get(a).unwrap().unwrap().status,
        FollowStatus::Deferred
    );
    r.key('a');
    assert_eq!(
        r.svc.get(b).unwrap().unwrap().status,
        FollowStatus::Approved
    );
    assert_eq!(r.svc.intents().unwrap().len(), 1);
}

#[test]
fn m_drafts_a_mission_from_the_selected_follow_up() {
    let mut r = rig();
    r.svc
        .ingest(
            {
                let mut f = FollowUp::new(
                    FollowKind::SuggestedTask,
                    "add retries to the fetch client",
                    Source::default(),
                    0,
                );
                f.paths = vec!["src/net.rs".into()];
                f
            },
            false,
        )
        .unwrap();
    r.key('9');
    r.t.app.refresh_followups();
    r.key('m');
    assert_eq!(r.t.app.screen, Screen::Missions);
    let composer = r.t.app.missions_screen.composer.as_ref().expect("composer");
    assert_eq!(composer.inputs[1].text, "add retries to the fetch client");
    assert_eq!(composer.inputs[3].text, "src/net.rs");
    assert_eq!(r.svc.intents().unwrap().len(), 1);
}
