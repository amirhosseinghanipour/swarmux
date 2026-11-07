use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use swarmux_domain::board::{BoardEntry, BoardKind};
use swarmux_domain::followup::{
    BriefSlots, DecidedBy, FollowKind, FollowStatus, FollowUp, OrderAction, Source, StandingOrder,
    Urgency,
};
use swarmux_domain::ledger::LedgerKind;
use swarmux_domain::{JobId, JobStatus, Millis, SessionId};

use super::*;
use crate::testing::{FixedClock, InMemoryLedger};

const H: Millis = 3_600_000;
const DAY0: Millis = 20_000 * 24 * H;

#[derive(Default)]
struct Mem {
    items: RefCell<Vec<FollowUp>>,
    state: RefCell<HashMap<String, i64>>,
    intents: RefCell<Vec<Intent>>,
}

impl FollowStore for Mem {
    fn insert(&self, f: &FollowUp) -> AppResult<i64> {
        let mut v = self.items.borrow_mut();
        let mut f = f.clone();
        f.id = v.len() as i64 + 1;
        v.push(f.clone());
        Ok(f.id)
    }
    fn update(&self, f: &FollowUp) -> AppResult<()> {
        let mut v = self.items.borrow_mut();
        if let Some(x) = v.iter_mut().find(|x| x.id == f.id) {
            *x = f.clone();
        }
        Ok(())
    }
    fn get(&self, id: i64) -> AppResult<Option<FollowUp>> {
        Ok(self.items.borrow().iter().find(|f| f.id == id).cloned())
    }
    fn all(&self) -> AppResult<Vec<FollowUp>> {
        Ok(self.items.borrow().clone())
    }
    fn state(&self, key: &str) -> AppResult<Option<i64>> {
        Ok(self.state.borrow().get(key).copied())
    }
    fn set_state(&self, key: &str, value: i64) -> AppResult<()> {
        self.state.borrow_mut().insert(key.into(), value);
        Ok(())
    }
}

impl IntentStore for Mem {
    fn add(&self, i: &Intent) -> AppResult<i64> {
        let mut v = self.intents.borrow_mut();
        let mut i = i.clone();
        i.id = v.len() as i64 + 1;
        v.push(i.clone());
        Ok(i.id)
    }
    fn list(&self) -> AppResult<Vec<Intent>> {
        Ok(self.intents.borrow().clone())
    }
    fn mark_mission(&self, id: i64, slug: &str) -> AppResult<()> {
        if let Some(i) = self.intents.borrow_mut().iter_mut().find(|i| i.id == id) {
            i.mission = Some(slug.into());
        }
        Ok(())
    }
}

#[derive(Default)]
struct FakeExtractor {
    asked: RefCell<Vec<(ExtractRequest, ExtractSink)>>,
}

impl FakeExtractor {
    fn answer(&self, i: usize, reply: ExtractReply) {
        let (_, sink) = self.asked.borrow_mut().remove(i);
        sink(reply);
    }
}

impl Extractor for FakeExtractor {
    fn extract(&self, r: ExtractRequest, sink: ExtractSink) {
        self.asked.borrow_mut().push((r, sink));
    }
}

#[derive(Default)]
struct Sent(RefCell<Vec<OwnerBrief>>);

impl BriefChannel for Sent {
    fn deliver(&self, b: &OwnerBrief) -> AppResult<()> {
        self.0.borrow_mut().push(b.clone());
        Ok(())
    }
}

struct Fx {
    svc: FollowUpService,
    mem: Rc<Mem>,
    clock: Rc<FixedClock>,
    ledger: Rc<InMemoryLedger>,
    extractor: Rc<FakeExtractor>,
    sent: Rc<Sent>,
}

fn fx(settings: FollowSettings) -> Fx {
    let mem = Rc::new(Mem::default());
    let clock = Rc::new(FixedClock::new(DAY0 + 6 * H));
    let ledger = Rc::new(InMemoryLedger::default());
    let extractor = Rc::new(FakeExtractor::default());
    let sent = Rc::new(Sent::default());
    let svc = FollowUpService::new(mem.clone(), mem.clone(), clock.clone(), settings)
        .with_ledger(Rc::new(LedgerService::new(ledger.clone(), clock.clone())))
        .with_extractor(extractor.clone())
        .with_channel(sent.clone());
    Fx {
        svc,
        mem,
        clock,
        ledger,
        extractor,
        sent,
    }
}

fn docs_order() -> StandingOrder {
    let mut o = StandingOrder::new("docs", OrderAction::AutoApprove);
    o.kinds = vec![FollowKind::SuggestedTask];
    o.paths = vec!["docs/**".into()];
    o
}

fn fact(id: i64, status: JobStatus, finished: Option<Millis>, result: &str) -> JobFact {
    JobFact {
        job_id: JobId(id),
        session_id: SessionId::random(),
        session_name: "s".into(),
        prompt: "do the thing".into(),
        model: "sonnet".into(),
        status,
        created_at: 0,
        started_at: None,
        last_event_at: None,
        finished_at: finished,
        error: None,
        result: Some(result.into()),
        review: None,
        review_of: None,
        cost_usd: 0.0,
        context: None,
        cache_ratio: None,
        flow: Vec::new(),
        todos_done: 0,
        todos_total: 0,
        loop_stops: 0,
        agents_running: 0,
        session_state: None,
    }
}

fn found(kind: FollowKind, text: &str, paths: &[&str]) -> Extracted {
    Extracted {
        kind,
        text: text.into(),
        rationale: "because".into(),
        against: String::new(),
        paths: paths.iter().map(|p| p.to_string()).collect(),
        urgency: Urgency::Normal,
        est_cost_usd: None,
    }
}

fn pump(f: &Fx, facts: &[JobFact], board: &[BoardEntry]) -> FollowPump {
    f.clock.advance(5_000);
    f.svc.pump(facts, &HashMap::new(), board)
}

fn board_entry(id: i64, kind: BoardKind, text: &str, after: Option<u32>, at: Millis) -> BoardEntry {
    let mut e = BoardEntry::new("m", JobId(1), kind, text, at);
    e.id = id;
    e.after_minutes = after;
    e
}

fn ledger_events(f: &Fx) -> Vec<String> {
    f.ledger
        .entries()
        .into_iter()
        .filter(|e| e.draft.kind == LedgerKind::Followup)
        .map(|e| e.draft.payload_json)
        .collect()
}

#[test]
fn a_board_followup_becomes_a_follow_up_and_its_delay_fires() {
    let f = fx(FollowSettings {
        orders: vec![docs_order()],
        ..FollowSettings::default()
    });
    let now = f.clock.now();
    let mut e = board_entry(7, BoardKind::Followup, "update the docs", Some(30), now);
    e.areas = vec!["docs/a.md".into()];
    let entries = vec![e];
    pump(&f, &[], &entries);
    let all = f.svc.all().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].kind, FollowKind::SuggestedTask);
    assert_eq!(all[0].status, FollowStatus::Deferred, "held for its delay");
    assert_eq!(all[0].sources[0].board_entry, Some(7));
    assert!(f.svc.waiting().unwrap().is_empty());

    pump(&f, &[], &entries);
    assert_eq!(f.svc.all().unwrap().len(), 1, "not made twice");

    f.clock.advance(31 * 60_000);
    pump(&f, &[], &entries);
    let all = f.svc.all().unwrap();
    // The order sees it when the delay is over.
    assert_eq!(all[0].status, FollowStatus::AutoApproved);
    assert_eq!(f.svc.intents().unwrap().len(), 1);
}

#[test]
fn a_board_question_is_a_question_follow_up_closed_when_answered() {
    let f = fx(FollowSettings::default());
    let now = f.clock.now();
    let mut q = board_entry(3, BoardKind::Question, "Postgres or SQLite?", None, now);
    pump(&f, &[], std::slice::from_ref(&q));
    let all = f.svc.all().unwrap();
    assert_eq!(all[0].kind, FollowKind::Question);
    assert_eq!(f.svc.waiting().unwrap().len(), 1);
    q.closed = true;
    pump(&f, &[], &[q]);
    assert_eq!(f.svc.all().unwrap()[0].status, FollowStatus::Done);
    assert!(f.svc.waiting().unwrap().is_empty());
}

#[test]
fn a_finished_job_is_read_once_and_its_follow_ups_are_deduplicated() {
    let f = fx(FollowSettings::default());
    let t = f.clock.now();
    pump(&f, &[], &[]); // starts the clock for "finished since"
    let jobs = vec![
        fact(1, JobStatus::Done, Some(t + 10_000), "Done. Also fix the typo in README."),
        fact(2, JobStatus::Done, Some(t + 12_000), "Done. I'd also fix the typo in README"),
        fact(3, JobStatus::Failed, Some(t + 12_000), "boom"),
        fact(4, JobStatus::Done, Some(t - 5 * H), "old job, before swarmux looked"),
    ];
    pump(&f, &jobs, &[]);
    assert_eq!(f.extractor.asked.borrow().len(), 2, "jobs 1 and 2 only");
    let req = f.extractor.asked.borrow()[0].0.clone();
    assert_eq!(req.job, JobId(1));
    assert_eq!(req.gate, "no gate verdict");
    let reply = |text: &str| ExtractReply {
        result: Ok(vec![found(FollowKind::SuggestedTask, text, &["README.md"])]),
        cost_usd: 0.002,
    };
    f.extractor.answer(0, reply("Fix the typo in README."));
    f.extractor.answer(0, reply("fix the typo in readme"));
    pump(&f, &jobs, &[]);
    let all = f.svc.all().unwrap();
    assert_eq!(all.len(), 1, "same kind, files and words are one follow-up");
    assert_eq!(all[0].jobs(), vec![JobId(1), JobId(2)]);
    // Never asked again for the same jobs.
    pump(&f, &jobs, &[]);
    assert!(f.extractor.asked.borrow().is_empty());
    // The model's cost is in the ledger.
    assert!(
        ledger_events(&f)
            .iter()
            .any(|p| p.contains("\"event\":\"extract\"") && p.contains("0.002"))
    );
}

#[test]
fn extraction_can_be_switched_off() {
    let f = fx(FollowSettings {
        extract: false,
        ..FollowSettings::default()
    });
    let t = f.clock.now();
    pump(&f, &[], &[]);
    pump(&f, &[fact(1, JobStatus::Done, Some(t + 1000), "x")], &[]);
    assert!(f.extractor.asked.borrow().is_empty());
}

#[test]
fn a_standing_order_approves_and_it_is_ledgered_but_decisions_still_wait() {
    let mut decisions = StandingOrder::new("anything", OrderAction::AutoApprove);
    decisions.kinds = vec![FollowKind::Decision];
    let f = fx(FollowSettings {
        orders: vec![docs_order(), decisions],
        ..FollowSettings::default()
    });
    let mut a = FollowUp::new(
        FollowKind::SuggestedTask,
        "fix the docs typo",
        Source::default(),
        0,
    );
    a.paths = vec!["docs/x.md".into()];
    let id = f.svc.ingest(a, false).unwrap();
    let got = f.svc.get(id).unwrap().unwrap();
    assert_eq!(got.status, FollowStatus::AutoApproved);
    assert_eq!(got.decided_by, Some(DecidedBy::Order("docs".into())));
    assert_eq!(f.svc.intents().unwrap().len(), 1);
    assert!(
        ledger_events(&f)
            .iter()
            .any(|p| p.contains("order:docs") && p.contains("auto-approved"))
    );

    let d = FollowUp::new(FollowKind::Decision, "REST or gRPC?", Source::default(), 0);
    let id = f.svc.ingest(d, false).unwrap();
    assert_eq!(f.svc.get(id).unwrap().unwrap().status, FollowStatus::Open);
}

#[test]
fn a_parked_follow_up_stays_out_of_the_brief() {
    let mut park = StandingOrder::new("later", OrderAction::Park);
    park.keywords = vec!["someday".into()];
    let f = fx(FollowSettings {
        orders: vec![park],
        ..FollowSettings::default()
    });
    let p = FollowUp::new(FollowKind::SuggestedTask, "someday rewrite", Source::default(), 0);
    f.svc.ingest(p, false).unwrap();
    assert!(f.svc.waiting().unwrap().is_empty());
    assert_eq!(f.svc.all().unwrap()[0].status, FollowStatus::Deferred);
}

#[test]
fn the_brief_is_sent_at_the_scheduled_times() {
    let f = fx(FollowSettings {
        slots: BriefSlots::parse(&["09:00", "13:00"]).unwrap(),
        ..FollowSettings::default()
    });
    // 06:00 UTC at the start.
    f.svc
        .ingest(
            FollowUp::new(FollowKind::Decision, "A or B?", Source::default(), 0),
            false,
        )
        .unwrap();
    let out = pump(&f, &[], &[]);
    assert!(out.briefs.is_empty(), "the first pump starts the clock");
    f.clock.advance(2 * H); // 08:00
    assert!(pump(&f, &[], &[]).briefs.is_empty());
    f.clock.advance(H + 5 * 60_000); // 09:05
    let out = pump(&f, &[], &[]);
    assert_eq!(out.briefs.len(), 1);
    assert_eq!(out.briefs[0].cards.len(), 1);
    assert_eq!(f.sent.0.borrow().len(), 1);
    f.clock.advance(H);
    assert!(pump(&f, &[], &[]).briefs.is_empty(), "once per slot");
    // At 13:00 the decision is still waiting, so it comes again.
    f.clock.advance(3 * H + 10 * 60_000);
    assert_eq!(pump(&f, &[], &[]).briefs.len(), 1);
    assert!(
        ledger_events(&f)
            .iter()
            .any(|p| p.contains("\"event\":\"brief\""))
    );
}

#[test]
fn nothing_waiting_and_nothing_finished_sends_no_brief() {
    let f = fx(FollowSettings::default());
    pump(&f, &[], &[]);
    f.clock.advance(12 * H);
    assert!(pump(&f, &[], &[]).briefs.is_empty());
}

#[test]
fn approving_leaves_an_intent_and_a_question_cannot_be_approved() {
    let f = fx(FollowSettings::default());
    let t = FollowUp::new(FollowKind::SuggestedTask, "add retries", Source::default(), 0);
    let id = f.svc.ingest(t, false).unwrap();
    let intent = f.svc.reply(id, Reply::Approve).unwrap().unwrap();
    assert_eq!(intent.text, "add retries");
    assert_eq!(intent.followup, id);
    let got = f.svc.get(id).unwrap().unwrap();
    assert_eq!(got.status, FollowStatus::Approved);
    assert_eq!(got.decided_by, Some(DecidedBy::Owner));
    f.svc.intent_became(intent.id, "retries").unwrap();
    assert_eq!(f.svc.intents().unwrap()[0].mission.as_deref(), Some("retries"));

    let q = FollowUp::new(FollowKind::Question, "which db?", Source::default(), 0);
    let qid = f.svc.ingest(q, false).unwrap();
    assert!(f.svc.reply(qid, Reply::Approve).is_err());
    assert!(f.svc.reply(qid, Reply::Decline).unwrap().is_none());
}

#[test]
fn deferring_hides_it_until_later_and_declining_keeps_it_from_returning() {
    let f = fx(FollowSettings::default());
    let a = FollowUp::new(FollowKind::SuggestedTask, "add retries", Source::default(), 0);
    let id = f.svc.ingest(a.clone(), false).unwrap();
    f.svc.reply(id, Reply::Defer).unwrap();
    assert!(f.svc.waiting().unwrap().is_empty());
    f.clock.advance(25 * H);
    pump(&f, &[], &[]);
    assert_eq!(f.svc.waiting().unwrap().len(), 1, "back after the delay");
    f.svc.reply(id, Reply::Decline).unwrap();
    f.svc.ingest(a, false).unwrap();
    assert_eq!(f.svc.all().unwrap().len(), 1);
    assert_eq!(f.svc.all().unwrap()[0].status, FollowStatus::Declined);
}

#[test]
fn only_urgent_follow_ups_reach_the_inbox() {
    let f = fx(FollowSettings::default());
    let mut risk = FollowUp::new(FollowKind::Risk, "key in the logs", Source::default(), 0);
    risk.urgency = Urgency::High;
    f.svc.ingest(risk, false).unwrap();
    f.svc
        .ingest(
            FollowUp::new(FollowKind::SuggestedTask, "tidy", Source::default(), 0),
            false,
        )
        .unwrap();
    let blocker = FollowUp::new(
        FollowKind::Blocker,
        "needs a migration",
        Source {
            job: Some(JobId(9)),
            ..Source::default()
        },
        0,
    );
    f.svc.ingest(blocker, true).unwrap();
    let mut running = fact(9, JobStatus::Running, None, "");
    running.result = None;
    pump(&f, &[running], &[]);
    let items = f.svc.urgent_items();
    assert_eq!(items.len(), 2);
    assert!(items.iter().any(|i| i.priority == swarmux_domain::Priority::P1));
    // The blocker stops being urgent when its job is no longer running.
    pump(&f, &[], &[]);
    assert_eq!(f.svc.urgent_items().len(), 1);
}

#[test]
fn twenty_owner_approvals_trigger_one_spot_check_in_the_brief_and_the_ledger() {
    let f = fx(FollowSettings::default());
    for i in 0..20 {
        let t = FollowUp::new(
            FollowKind::SuggestedTask,
            &format!("distinct task number {i} about module{i}"),
            Source::default(),
            0,
        );
        let id = f.svc.ingest(t, false).unwrap();
        f.clock.advance(1000);
        f.svc.reply(id, Reply::Approve).unwrap();
    }
    let brief = f.svc.brief_now(&[], &HashMap::new()).unwrap();
    assert!(brief.spot_check.is_some());
    assert!(brief.spot_note().unwrap().contains("20 of your last 20"));
    assert!(ledger_events(&f).iter().any(|p| p.contains("spot_check")));
    // Not again right away.
    let again = f.svc.brief_now(&[], &HashMap::new()).unwrap();
    assert!(again.spot_check.is_none());
}

#[test]
fn a_standing_order_approval_is_not_counted_as_the_owners() {
    let mut o = StandingOrder::new("t", OrderAction::AutoApprove);
    o.kinds = vec![FollowKind::SuggestedTask];
    let f = fx(FollowSettings {
        orders: vec![o],
        ..FollowSettings::default()
    });
    for i in 0..25 {
        let t = FollowUp::new(
            FollowKind::SuggestedTask,
            &format!("distinct task {i} module{i}"),
            Source::default(),
            0,
        );
        f.svc.ingest(t, false).unwrap();
    }
    let brief = f.svc.brief_now(&[], &HashMap::new()).unwrap();
    assert!(brief.stamped.is_empty());
}

#[test]
fn outside_replies_are_applied() {
    struct Chat(RefCell<Vec<OwnerReply>>);
    impl ReplySource for Chat {
        fn poll(&self) -> Vec<OwnerReply> {
            std::mem::take(&mut *self.0.borrow_mut())
        }
    }
    let mut f = fx(FollowSettings::default());
    let id = f
        .svc
        .ingest(
            FollowUp::new(FollowKind::SuggestedTask, "add retries", Source::default(), 0),
            false,
        )
        .unwrap();
    let chat = Rc::new(Chat(RefCell::new(vec![OwnerReply {
        followup: id,
        reply: Reply::Decline,
    }])));
    f.svc = f.svc.with_replies(chat);
    pump(&f, &[], &[]);
    assert_eq!(f.svc.get(id).unwrap().unwrap().status, FollowStatus::Declined);
    let _ = &f.mem;
}
