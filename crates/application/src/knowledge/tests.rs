use std::path::Path;
use std::rc::Rc;

use swarmux_domain::knowledge::{
    Action, Anchor, CheckRule, Draft, Enforcer, GuardRule, Home, Kind, Source, Status, Trust,
};
use swarmux_domain::ledger::LedgerKind;
use swarmux_domain::{
    AttentionKind, GuardPolicy, JobId, Pattern, Priority, Session, SessionId, Unmatched, Verdict,
};

use super::*;
use crate::hookbus::{SessionStartContext, SessionStartInfo};
use crate::ports::HookRequest;
use crate::testing::{
    FakeAnchors, FakeDecisionRepo, FakeHookInbox, FixedClock, InMemoryKnowledge, InMemoryLedger,
};
use crate::{AttentionService, GuardService, SharedPolicy};

struct Rig {
    service: KnowledgeService,
    store: Rc<InMemoryKnowledge>,
    ledger: Rc<InMemoryLedger>,
    files: Rc<FakeAnchors>,
    clock: Rc<FixedClock>,
}

fn home() -> Home {
    Home::repo("/w")
}

fn rig() -> Rig {
    let store = Rc::new(InMemoryKnowledge::default());
    let ledger = Rc::new(InMemoryLedger::default());
    let files = Rc::new(FakeAnchors::default());
    let clock = Rc::new(FixedClock::new(1_000));
    let service = KnowledgeService::new(store.clone(), clock.clone())
        .unwrap()
        .with_ledger(Rc::new(LedgerService::new(ledger.clone(), clock.clone())))
        .with_reader(files.clone());
    Rig {
        service,
        store,
        ledger,
        files,
        clock,
    }
}

fn guard(paths: &[&str], pattern: Option<&str>, action: Action) -> Enforcer {
    Enforcer::Guard(GuardRule {
        tools: Vec::new(),
        paths: paths.iter().map(|s| s.to_string()).collect(),
        pattern: pattern.map(str::to_string),
        action,
        message: String::new(),
    })
}

fn rule(text: &str, enforcer: Enforcer, status: Status) -> Draft {
    let mut d = Draft::new(home(), Kind::Rule, text);
    d.enforcer = Some(enforcer);
    d.status = Some(status);
    d
}

fn session() -> Session {
    Session::new(SessionId::random(), "w", "/w".into(), "sonnet", 0).unwrap()
}

/// Tells the service a job of a plain session at `/w` started.
fn start(r: &Rig, job: i64) -> JobId {
    let job = JobId(job);
    let s = session();
    let start = JobStart {
        job,
        session: &s,
        cwd: Path::new("/w"),
        repos: Vec::new(),
        review: false,
    };
    r.service.started(&start).unwrap();
    job
}

fn request(job: JobId, id: u64, tool: &str, subject: &str, content: &str) -> HookRequest {
    HookRequest {
        request_id: id,
        job_id: Some(job),
        tool_name: tool.into(),
        subject: subject.into(),
        input: String::new(),
        cwd: "/w".into(),
        content: content.into(),
    }
}

fn knowledge_entries(r: &Rig) -> Vec<String> {
    r.ledger
        .entries()
        .into_iter()
        .filter(|e| e.draft.kind == LedgerKind::Knowledge)
        .map(|e| e.draft.payload_json)
        .collect()
}

#[test]
fn the_projection_rebuilds_from_its_events() {
    let r = rig();
    r.files.set("db/auth.sql", "anon users are users\n");
    let mut d = rule(
        "Anonymous users are full users.",
        guard(&["db/**"], Some("is_anonymous"), Action::Ask),
        Status::Shadow,
    );
    d.why = "the backend relies on it".into();
    d.anchors = vec![Anchor::parse("db/auth.sql")];
    let r1 = r.service.add(d.clone()).unwrap();
    assert_eq!(r1.id.as_str(), "R1");
    assert!(!r1.anchors[0].hash.is_empty(), "hashed on save");
    let f1 = r
        .service
        .add(Draft::new(home(), Kind::Fact, "The app is a client."))
        .unwrap();
    r.service.promote(&r1.id).unwrap();
    d.text = "Anonymous (guest) users are full users.".into();
    d.status = None;
    r.service.edit(&r1.id, d).unwrap();
    r.files.set("db/auth.sql", "anon users are not users\n");
    assert_eq!(r.service.recheck(&home()).unwrap(), vec![r1.id.clone()]);
    r.clock.advance(5);
    r.service.confirm(&r1.id).unwrap();
    r.service.retire(&f1.id).unwrap();
    let live = r.service.list();
    let now = r.service.get(&r1.id).unwrap();
    assert_eq!(now.status, Status::Enforced, "promoted, stale, confirmed");
    assert_eq!(now.text, "Anonymous (guest) users are full users.");

    // Every change is in the ledger, and the item names its entries.
    let entries = knowledge_entries(&r);
    assert_eq!(entries.len(), r.store.events.borrow().len());
    assert!(entries[0].contains("\"event\":\"put\""));
    assert_eq!(now.provenance.events.len(), 5);

    // A fresh fold of the stored events is the live view.
    let again = KnowledgeService::new(r.store.clone(), r.clock.clone()).unwrap();
    assert_eq!(again.list(), live);
    let rebuilt = r.service.rebuild().unwrap();
    assert!(rebuilt.differed.is_empty(), "{rebuilt:?}");
    assert_eq!((rebuilt.items, rebuilt.events), (2, 7));

    // A damaged stored projection is put right.
    r.store.items.borrow_mut().get_mut("R1").unwrap().text = "forged".into();
    r.store.items.borrow_mut().remove("F1");
    let rebuilt = r.service.rebuild().unwrap();
    let ids: Vec<&str> = rebuilt.differed.iter().map(ItemId::as_str).collect();
    assert_eq!(ids, ["F1", "R1"]);
    assert_eq!(r.store.items.borrow()["R1"].text, now.text);
}

#[test]
fn moves_that_make_no_sense_are_refused() {
    let r = rig();
    let f = r
        .service
        .add(Draft::new(home(), Kind::Fact, "a fact"))
        .unwrap();
    assert!(r.service.set_status(&f.id, Status::Enforced, "").is_err());
    assert!(
        r.service.promote(&f.id).is_err(),
        "advisory is as far as facts go"
    );
    let mut proposed = Draft::new(home(), Kind::Rule, "drafted by a tool");
    proposed.source = Source::Gate;
    proposed.enforcer = Some(guard(&[], None, Action::Deny));
    let p = r.service.add(proposed).unwrap();
    assert_eq!(p.status, Status::Proposed);
    assert_eq!(r.service.promote(&p.id).unwrap().status, Status::Shadow);
    assert_eq!(r.service.promote(&p.id).unwrap().status, Status::Enforced);
    assert!(
        r.service
            .add(Draft::new(home(), Kind::Lesson, "x"))
            .is_err(),
        "reserved"
    );
}

#[test]
fn guard_rules_deny_ask_warn_and_shadow_only_logs() {
    let r = rig();
    r.service
        .add(rule(
            "Never drop tables.",
            guard(&[], Some("DROP TABLE"), Action::Deny),
            Status::Enforced,
        ))
        .unwrap();
    r.service
        .add(rule(
            "Ask before auth changes.",
            guard(&["db/**"], None, Action::Ask),
            Status::Enforced,
        ))
        .unwrap();
    r.service
        .add(rule(
            "Leave no TODOs.",
            guard(&["app/**"], Some("TODO"), Action::Warn),
            Status::Enforced,
        ))
        .unwrap();
    let shadow = r
        .service
        .add(rule(
            "Don't wipe the build.",
            guard(&[], Some("rm -rf build"), Action::Deny),
            Status::Shadow,
        ))
        .unwrap();
    let job = start(&r, 7);
    let hub = r.service.hub();
    let policy = SharedPolicy::new(GuardPolicy::new(
        vec![
            Pattern::parse("Bash").unwrap(),
            Pattern::parse("Edit").unwrap(),
        ],
        vec![Pattern::parse("Write(db/secret.sql)").unwrap()],
        Unmatched::Ask,
    ))
    .with_rules(hub.clone());

    let (d, _) = policy.decide(&request(job, 1, "Bash", "psql -c 'DROP TABLE users'", ""));
    assert_eq!(
        d.verdict,
        Verdict::Deny,
        "the rule forbids what the policy allows"
    );
    assert!(
        d.rule
            .unwrap()
            .starts_with("knowledge rule R1: Never drop tables.")
    );

    let (d, _) = policy.decide(&request(job, 2, "Write", "/w/db/auth.sql", "x"));
    assert_eq!(d.verdict, Verdict::Ask);
    assert!(d.rule.unwrap().contains("R2"));

    let (d, _) = policy.decide(&request(job, 3, "Write", "db/secret.sql", "x"));
    assert_eq!(
        d.verdict,
        Verdict::Deny,
        "the policy's deny beats a rule's ask"
    );
    assert!(!d.rule.unwrap().contains("knowledge"));

    let warn = request(job, 4, "Edit", "app/main.dart", "// TODO later");
    let (d, _) = policy.decide(&warn);
    assert_eq!(d.verdict, Verdict::Allow, "a warning lets it through");
    let ctx = hub.tool_context(&warn).unwrap();
    assert!(
        ctx.contains("SWARMUX RULE (warning): [R3 rule · enforced · owner]"),
        "{ctx}"
    );

    let (d, _) = policy.decide(&request(job, 5, "Bash", "rm -rf build", ""));
    assert_eq!(d.verdict, Verdict::Allow, "shadow rules don't act");
    let (d, _) = policy.decide(&request(job, 6, "Bash", "ls", ""));
    assert_eq!(d.verdict, Verdict::Allow);
    // A call of an unknown job meets no rule.
    let mut stranger = request(job, 7, "Bash", "DROP TABLE x", "");
    stranger.job_id = Some(JobId(99));
    assert_eq!(policy.decide(&stranger).0.verdict, Verdict::Allow);

    assert!(r.service.pump().is_empty());
    let entries = knowledge_entries(&r);
    assert!(
        entries
            .iter()
            .any(|e| e.contains("\"action\":\"would_deny\"") && e.contains("R4"))
    );
    assert!(
        entries
            .iter()
            .any(|e| e.contains("\"event\":\"rule\"") && e.contains("\"action\":\"deny\""))
    );
    let s = r.service.get(&shadow.id).unwrap();
    assert_eq!((s.counters.shadow_hits, s.counters.hits), (1, 0));
    assert_eq!(
        r.service
            .get(&ItemId::parse("R1").unwrap())
            .unwrap()
            .counters
            .hits,
        1
    );
}

#[test]
fn a_rule_that_asks_is_a_p1_approval_showing_the_rule() {
    let r = rig();
    r.service
        .add(rule(
            "Ask before auth changes.",
            guard(&["db/**"], None, Action::Ask),
            Status::Enforced,
        ))
        .unwrap();
    let job = start(&r, 3);
    let policy = SharedPolicy::new(GuardPolicy::new(
        vec![Pattern::parse("Edit").unwrap()],
        Vec::new(),
        Unmatched::Pass,
    ))
    .with_rules(r.service.hub());
    let inbox = Rc::new(FakeHookInbox::default());
    let mut guard = GuardService::new(
        policy,
        inbox.clone(),
        Rc::new(FakeDecisionRepo::default()),
        r.clock.clone(),
    );
    inbox.push(Some(job), "Edit", "db/auth.sql");
    guard.pump();
    let held = guard.approvals();
    assert_eq!(held.len(), 1);
    assert!(
        held[0]
            .rule
            .as_deref()
            .unwrap()
            .contains("Ask before auth changes.")
    );
    let mut attention = AttentionService::new();
    let items = attention.collect_from(&held, &[], 2_000);
    assert_eq!(items[0].kind, AttentionKind::Approval);
    assert_eq!(items[0].priority, Priority::P1);
    assert!(items[0].detail.starts_with("knowledge rule R1"));
    // Decided twice (transport, then app): logged once.
    r.service.pump();
    let n = knowledge_entries(&r)
        .iter()
        .filter(|e| e.contains("\"event\":\"rule\""))
        .count();
    assert_eq!(n, 1);
}

#[test]
fn sessions_get_the_core_at_start_and_again_after_compaction() {
    let r = rig();
    let saved = r
        .service
        .save_map(
            &home(),
            &[
                "The product is a marketplace.".into(),
                "The app calls the API.".into(),
            ],
        )
        .unwrap();
    assert_eq!(saved.added, 2);
    r.service
        .add(rule(
            "Anonymous users are full users.",
            Enforcer::Prompt,
            Status::Enforced,
        ))
        .unwrap();
    let job = start(&r, 1);
    let hub = r.service.hub();
    let info = |source: &str| SessionStartInfo {
        job: Some(job),
        session: "c".into(),
        source: source.into(),
    };
    let first = hub.context(&info("startup")).unwrap();
    assert!(first.contains("Project map:"));
    assert!(first.contains("\"Anonymous users are full users.\""));
    let after = hub.context(&info("compact")).unwrap();
    assert_eq!(after, first, "the same block, so it stays cached");
    assert!(
        hub.context(&info("resume")).is_none(),
        "already in the transcript"
    );
    assert!(
        hub.context(&SessionStartInfo {
            job: None,
            ..info("startup")
        })
        .is_none(),
        "not a swarmux job"
    );
    r.service.pump();
    let injected: Vec<String> = knowledge_entries(&r)
        .into_iter()
        .filter(|e| e.contains("\"event\":\"injected\""))
        .collect();
    assert_eq!(injected.len(), 2);
    assert!(injected[1].contains("\"trigger\":\"compact\""));
    assert!(injected[0].contains("\"items\":[\"F1\",\"F2\",\"R1\"]"));
    assert_eq!(
        r.service
            .get(&ItemId::parse("R1").unwrap())
            .unwrap()
            .counters
            .hits,
        2
    );

    // The map edited again: kept, added, retired.
    let saved = r
        .service
        .save_map(
            &home(),
            &[
                "The product is a marketplace.".into(),
                "Admin is internal.".into(),
            ],
        )
        .unwrap();
    assert_eq!((saved.kept, saved.added, saved.retired), (1, 1, 1));
}

#[test]
fn the_board_digest_follows_the_knowledge() {
    struct Inner;
    impl SessionStartContext for Inner {
        fn context(&self, _: &SessionStartInfo) -> Option<String> {
            Some("HANDOFF".into())
        }
    }
    let r = rig();
    r.service
        .add(rule("Be kind.", Enforcer::Prompt, Status::Enforced))
        .unwrap();
    let job = start(&r, 1);
    let hub = r.service.hub();
    hub.set_inner(Arc::new(Inner));
    let text = hub
        .context(&SessionStartInfo {
            job: Some(job),
            session: "c".into(),
            source: "startup".into(),
        })
        .unwrap();
    assert!(text.starts_with("SWARMUX KNOWLEDGE"));
    assert!(text.ends_with("\n\nHANDOFF"));
}

#[test]
fn pulls_come_once_per_item_per_job_within_the_cap() {
    let r = rig();
    let mut scoped = rule(
        "Migrations are append-only.",
        Enforcer::Prompt,
        Status::Enforced,
    );
    scoped.scope.paths = vec!["db/migrations/**".into()];
    r.service.add(scoped).unwrap();
    for n in 0..30 {
        let mut d = Draft::new(
            home(),
            Kind::Fact,
            &format!("fact {n}: {}", "detail ".repeat(10)),
        );
        d.scope.paths = vec!["db/**".into()];
        r.service.add(d).unwrap();
    }
    let job = start(&r, 1);
    let other = start(&r, 2);
    let hub = r.service.hub();
    let read = |job, path: &str| hub.tool_context(&request(job, 1, "Read", path, ""));
    let first = read(job, "/w/db/migrations/001.sql").unwrap();
    assert!(
        first.contains("[R1 rule · advisory · owner]"),
        "rules first"
    );
    assert!(swarmux_domain::estimate_tokens(&first) <= swarmux_domain::knowledge::PULL_TOKENS);
    let second = read(job, "/w/db/migrations/001.sql").unwrap();
    assert!(!second.contains("R1 rule"), "once per job");
    assert!(
        read(other, "db/migrations/001.sql")
            .unwrap()
            .contains("R1 rule"),
        "per job"
    );
    assert!(
        read(job, "/elsewhere/db/x.sql").is_none(),
        "outside the root"
    );
    assert!(
        read(job, "/w/app/main.dart").is_none(),
        "nothing scoped there"
    );
    assert!(
        hub.tool_context(&request(job, 1, "Bash", "cat db/x", ""))
            .is_none()
    );
    // Everything pulled is pulled again after a compaction.
    hub.context(&SessionStartInfo {
        job: Some(job),
        session: "c".into(),
        source: "compact".into(),
    });
    assert!(
        read(job, "db/migrations/001.sql")
            .unwrap()
            .contains("R1 rule")
    );
    r.service.pump();
    assert!(
        knowledge_entries(&r)
            .iter()
            .any(|e| e.contains("\"trigger\":\"pull\""))
    );
}

#[test]
fn stale_untrusted_proposed_and_retired_items_never_reach_agents() {
    let r = rig();
    r.files.set("db/auth.sql", "v1");
    let mut anchored = rule("Anchored rule.", Enforcer::Prompt, Status::Enforced);
    anchored.anchors = vec![Anchor::parse("db/auth.sql")];
    let anchored = r.service.add(anchored).unwrap();
    let mut proposed = Draft::new(home(), Kind::Fact, "An agent's guess.");
    proposed.source = Source::AgentAsk;
    proposed.core = true;
    r.service.add(proposed).unwrap();
    let mut trusted_later = Draft::new(home(), Kind::Fact, "Told by an agent.");
    trusted_later.source = Source::AgentAsk;
    trusted_later.core = true;
    trusted_later.status = Some(Status::Advisory);
    let untrusted = r.service.add(trusted_later).unwrap();
    assert_eq!(untrusted.trust, Trust::Untrusted);
    let gone = r
        .service
        .add(rule("Old rule.", Enforcer::Prompt, Status::Enforced))
        .unwrap();
    r.service.retire(&gone.id).unwrap();
    let hub = r.service.hub();

    // The file changed before the job started: the start rechecks it.
    r.files.set("db/auth.sql", "v2");
    let job = start(&r, 1);
    assert_eq!(r.service.get(&anchored.id).unwrap().status, Status::Stale);
    assert!(hub.pushed_for(job).is_none(), "nothing deliverable");
    assert!(
        hub.tool_context(&request(job, 1, "Read", "db/auth.sql", ""))
            .is_none()
    );
    let inbox = r.service.stale_items();
    assert_eq!(inbox.len(), 1);
    assert_eq!(
        (inbox[0].kind, inbox[0].priority),
        (AttentionKind::Knowledge, Priority::P3)
    );
    assert!(inbox[0].title.contains("R1"));

    // Confirmed and trusted, both reach the job.
    r.service.confirm(&anchored.id).unwrap();
    r.service.trust(&untrusted.id, Trust::Verified).unwrap();
    let block = hub.pushed_for(job).unwrap();
    let ids: Vec<&str> = block.items.iter().map(ItemId::as_str).collect();
    assert_eq!(ids, ["F2", "R1"]);
    assert!(r.service.stale_items().is_empty());
}

#[test]
fn gate_and_lint_rules_become_checks_for_jobs_in_scope() {
    let r = rig();
    r.service
        .add(rule(
            "Guests are users.",
            Enforcer::Gate(CheckRule {
                command: "psql -f anon.sql".into(),
            }),
            Status::Enforced,
        ))
        .unwrap();
    r.service
        .add(rule(
            "Analyze clean.",
            Enforcer::Lint(CheckRule {
                command: "flutter analyze".into(),
            }),
            Status::Shadow,
        ))
        .unwrap();
    let mut elsewhere = rule(
        "Only in the admin.",
        Enforcer::Lint(CheckRule {
            command: "npm run lint".into(),
        }),
        Status::Enforced,
    );
    elsewhere.scope.classes = vec!["review".into()];
    r.service.add(elsewhere).unwrap();
    let job = start(&r, 1);
    let hub = r.service.hub();
    let plain = hub.checks(job, false);
    assert_eq!(plain.len(), 1, "hidden checks are for missions");
    assert!(plain[0].shadow && !plain[0].hidden);
    let mission = hub.checks(job, true);
    assert_eq!(mission.len(), 2);
    assert_eq!(
        mission[0].check.name, "rule R1",
        "a hidden check names nothing"
    );
    hub.report(job, &mission[0].rule, false, false);
    r.service.pump();
    let r1 = r.service.get(&ItemId::parse("R1").unwrap()).unwrap();
    assert_eq!(r1.last_check, Some(false));
    assert_eq!(r.service.registry(None).failing, 1);
    assert_eq!(
        r.service.registry(Some(&home())).to_string(),
        "3 rules · 2 enforced · 1 failing · 0 advisory-only"
    );
}

#[test]
fn mission_jobs_gain_their_class() {
    let r = rig();
    let mut d = rule("Mission rule.", Enforcer::Prompt, Status::Enforced);
    d.scope.classes = vec!["mission".into()];
    r.service.add(d).unwrap();
    let job = start(&r, 1);
    let hub = r.service.hub();
    assert!(hub.pushed_for(job).is_none());
    let m = Mission::new("m", "/w", swarmux_domain::mission::Contract::default(), 0);
    r.service.launched(job, &m);
    assert!(hub.pushed_for(job).is_some());
}

#[test]
fn edits_made_elsewhere_are_folded_in_by_the_pump() {
    let r = rig();
    let other = KnowledgeService::new(r.store.clone(), r.clock.clone()).unwrap();
    other
        .add(Draft::new(
            home(),
            Kind::Fact,
            "added from the command line",
        ))
        .unwrap();
    assert!(r.service.list().is_empty());
    assert!(r.service.pump().is_empty());
    assert_eq!(r.service.list().len(), 1);
}

#[test]
fn imports_keep_ids_and_count_what_changed() {
    let r = rig();
    let f = r
        .service
        .add(Draft::new(home(), Kind::Fact, "one"))
        .unwrap();
    let mut items = r.service.export(&home());
    let done = r.service.import(items.clone()).unwrap();
    assert_eq!((done.added, done.changed, done.unchanged), (0, 0, 1));
    items[0].text = "one, edited in git".into();
    let mut new = items[0].clone();
    new.id = ItemId::parse("F9").unwrap();
    new.text = "nine".into();
    items.push(new);
    let done = r.service.import(items).unwrap();
    assert_eq!((done.added, done.changed), (1, 1));
    assert_eq!(r.service.get(&f.id).unwrap().text, "one, edited in git");
    assert!(r.service.get(&ItemId::parse("F9").unwrap()).is_some());
}
