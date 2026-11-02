use super::*;
use crate::ports::{AdviceAnswer, AdviceError, AdviceRequest, NewJob};
use crate::testing::{
    FakeAdvisor, FakeConsents, FakeDecisionRepo, FakeRouter, FixedClock, InMemoryStore, advice,
    picks,
};

struct Fixture {
    svc: RoutingService,
    store: Rc<InMemoryStore>,
    router: Rc<FakeRouter>,
    decisions: Rc<FakeDecisionRepo>,
    clock: Rc<FixedClock>,
    sessions: Vec<Session>,
}

fn fixture(names: &[&str]) -> Fixture {
    let store = Rc::new(InMemoryStore::default());
    let clock = Rc::new(FixedClock::new(1_000));
    let mut sessions = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let s = Session::new(
            SessionId::random(),
            name,
            format!("/w/{name}").into(),
            "sonnet",
            i as i64,
        )
        .unwrap();
        SessionRepository::insert(store.as_ref(), &s).unwrap();
        sessions.push(s);
    }
    let router = Rc::new(FakeRouter::default());
    let decisions = Rc::new(FakeDecisionRepo::default());
    let svc = RoutingService::new(
        store.clone(),
        store.clone(),
        decisions.clone(),
        clock.clone(),
    )
    .with_router(router.clone());
    Fixture {
        svc,
        store,
        router,
        decisions,
        clock,
        sessions,
    }
}

fn with_jev(f: Fixture, consented: bool) -> (Fixture, Rc<FakeAdvisor>) {
    let advisor = Rc::new(FakeAdvisor::default());
    let jev = Jev::new(
        advisor.clone(),
        Rc::new(FakeConsents::default()),
        f.clock.clone(),
        0.85,
    )
    .unwrap();
    if consented {
        jev.grant().unwrap();
    }
    let svc = f.svc.with_jev(Rc::new(jev));
    (Fixture { svc, ..f }, advisor)
}

#[test]
fn haiku_picks_a_session_with_reason_and_cost() {
    let mut f = fixture(&["api", "web"]);
    let ticket = f
        .svc
        .start("fix the login button CSS", f.sessions[0].id)
        .unwrap();
    assert_eq!(f.svc.in_flight(), 1);
    assert!(f.svc.pump().routed.is_empty(), "still routing");

    let request = &f.router.requests()[0];
    assert_eq!(request.prompt, "fix the login button CSS");
    let names: Vec<_> = request.candidates.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["web", "api"]);
    assert_eq!(request.candidates[0].cwd, "/w/web");

    f.clock.advance(1_500);
    f.router
        .reply(0, picks("web", Some("haiku"), "front-end change"));
    let report = f.svc.pump();
    let routed = &report.routed[0];
    assert_eq!(routed.ticket, ticket);
    assert_eq!(routed.session.name, "web");
    assert_eq!(routed.model_hint.as_deref(), Some("haiku"));
    assert_eq!(routed.source, Source::Haiku);
    assert_eq!(routed.cost_usd, 0.0002);
    assert_eq!(
        routed.summary(),
        "routed → web (haiku, suggests haiku, $0.0002): front-end change"
    );
    let row = f.decisions.rows().pop().unwrap();
    assert_eq!((row.outcome, row.source), (Outcome::Route, Source::Haiku));
    assert_eq!(
        row.rule.as_deref(),
        Some("→ web (suggests haiku, $0.0002): front-end change")
    );
    assert_eq!(row.latency_us, 1_500_000);
    assert_eq!(row.confidence, None);
    assert_eq!(f.svc.recent().count(), 1);
    assert_eq!(f.svc.in_flight(), 0);
}

#[test]
fn router_sees_each_sessions_latest_prompts() {
    let mut f = fixture(&["api", "web"]);
    for (i, p) in ["old", "newer", "newest", "latest"].iter().enumerate() {
        let job = NewJob {
            session_id: f.sessions[0].id,
            prompt_id: None,
            prompt: p.to_string(),
            model: "sonnet".into(),
            priority: 0,
            fork: false,
            wants_review: false,
            review_of: None,
        };
        JobRepository::insert(f.store.as_ref(), &job, 10 + i as i64).unwrap();
    }
    f.svc.start("x", f.sessions[0].id).unwrap();
    let request = &f.router.requests()[0];
    let api = request.candidates.iter().find(|c| c.name == "api").unwrap();
    assert_eq!(api.recent, ["latest", "newest", "newer"]);
}

#[test]
fn long_prompts_are_cut_for_the_router() {
    let mut f = fixture(&["api", "web"]);
    f.svc.start(&"x".repeat(5000), f.sessions[0].id).unwrap();
    assert_eq!(
        f.router.requests()[0].prompt.chars().count(),
        ROUTE_PROMPT_MAX
    );
}

#[test]
fn bad_or_failed_picks_keep_the_selected_session() {
    let mut f = fixture(&["api", "web"]);
    let selected = f.sessions[0].id;
    f.svc.start("x", selected).unwrap();
    f.router.reply(0, picks("nope", None, "?"));
    let routed = f.svc.pump().routed.remove(0);
    assert_eq!(routed.session.id, selected);
    assert_eq!(routed.source, Source::HaikuError);
    assert!(
        routed.reason.contains("unknown session \"nope\""),
        "{}",
        routed.reason
    );
    assert_eq!(routed.cost_usd, 0.0002, "the call still cost something");

    f.svc.start("y", selected).unwrap();
    f.router.reply(
        1,
        RouteReply {
            result: Err("claude exited with code 1".into()),
            cost_usd: 0.0,
        },
    );
    let routed = f.svc.pump().routed.remove(0);
    assert_eq!(routed.session.id, selected);
    assert!(routed.reason.contains("claude exited"));
}

#[test]
fn names_match_ignoring_case() {
    let mut f = fixture(&["api", "Web"]);
    f.svc.start("x", f.sessions[0].id).unwrap();
    f.router.reply(0, picks(" web ", None, "ui"));
    assert_eq!(f.svc.pump().routed[0].session.name, "Web");
}

#[test]
fn one_session_needs_no_router() {
    let mut f = fixture(&["api"]);
    f.svc.start("x", f.sessions[0].id).unwrap();
    let routed = f.svc.pump().routed.remove(0);
    assert_eq!(routed.source, Source::Fallback);
    assert_eq!(routed.reason, "the only session");
    assert!(f.router.requests().is_empty());
}

#[test]
fn no_sessions_is_an_error() {
    let mut f = fixture(&[]);
    assert!(matches!(
        f.svc.start("x", SessionId::random()),
        Err(AppError::SessionNotFound)
    ));
}

#[test]
fn confident_jev_skips_haiku() {
    let (mut f, advisor) = with_jev(fixture(&["api", "web"]), true);
    f.svc.start("deploy the api", f.sessions[1].id).unwrap();
    let AdviceRequest::Session { prompt, candidates } = &advisor.requests()[0] else {
        panic!()
    };
    assert_eq!(prompt, "deploy the api");
    assert_eq!(candidates, &["web", "api"]);
    advisor.reply(0, advice(AdviceAnswer::Session("api".into()), 0.91));
    let routed = f.svc.pump().routed.remove(0);
    assert_eq!(routed.session.name, "api");
    assert_eq!(
        (routed.source, routed.confidence),
        (Source::Jev, Some(0.91))
    );
    assert!(f.router.requests().is_empty());
    let row = f.decisions.rows().pop().unwrap();
    assert_eq!(row.confidence, Some(0.91));
}

#[test]
fn unsure_or_failing_jev_hands_over_to_haiku() {
    let (mut f, advisor) = with_jev(fixture(&["api", "web"]), true);
    f.svc.start("x", f.sessions[0].id).unwrap();
    advisor.reply(0, advice(AdviceAnswer::Session("api".into()), 0.5));
    assert!(f.svc.pump().routed.is_empty());
    assert_eq!(f.router.requests().len(), 1);
    let row = f.decisions.rows().pop().unwrap();
    assert_eq!((row.source, row.confidence), (Source::Jev, Some(0.5)));
    f.router.reply(0, picks("web", None, "ui"));
    assert_eq!(f.svc.pump().routed[0].source, Source::Haiku);

    f.svc.start("y", f.sessions[0].id).unwrap();
    advisor.reply(1, Err(AdviceError::Status(401)));
    f.svc.pump();
    let row = f.decisions.rows().pop().unwrap();
    assert_eq!(row.source, Source::JevError);
    assert!(row.rule.unwrap().contains("HTTP 401"));
    assert_eq!(f.router.requests().len(), 2);
}

#[test]
fn jev_without_consent_is_not_asked() {
    let (mut f, advisor) = with_jev(fixture(&["api", "web"]), false);
    f.svc.start("x", f.sessions[0].id).unwrap();
    assert!(advisor.requests().is_empty());
    assert_eq!(f.router.requests().len(), 1);
}

#[test]
fn without_a_router_the_selected_session_is_kept() {
    let store = Rc::new(InMemoryStore::default());
    let clock = Rc::new(FixedClock::new(0));
    for name in ["a", "b"] {
        let s = Session::new(SessionId::random(), name, "/w".into(), "sonnet", 0).unwrap();
        SessionRepository::insert(store.as_ref(), &s).unwrap();
    }
    let selected = store.list_active().unwrap()[1].id;
    let mut svc = RoutingService::new(
        store.clone(),
        store,
        Rc::new(FakeDecisionRepo::default()),
        clock,
    );
    svc.start("x", selected).unwrap();
    let routed = svc.pump().routed.remove(0);
    assert_eq!(
        (routed.session.id, routed.reason.as_str()),
        (selected, "no router")
    );
}
