use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use swarmux_domain::ledger::LedgerKind;
use swarmux_domain::{
    Actor, AgentEvent, AttentionKind, CheckpointReason, JobId, JobStatus, Millis, Rung,
    SessionState, Verdict,
};

use super::*;
use crate::guard::SharedPolicy;
use crate::ports::{AdviceRequest, HookRequest, RunnerMsg};
use crate::testing::{
    FakeAdvisor, FakeConsents, FakeDiagnoser, FakeRestore, FakeRunner, WorktreeRig, advice,
    diagnosis,
};
use crate::{GuardSettings, Settings};

pub(super) const SEC: Millis = 1_000;
pub(super) const MIN: Millis = 60 * SEC;

/// A job service on fakes with a watchdog on top, and a guard policy that
/// hands out its nudges the way the hook transport does.
pub(super) struct Rig {
    pub wt: WorktreeRig,
    pub runner: Rc<FakeRunner>,
    pub jobs: JobService,
    pub dog: Watchdog,
    pub board: Arc<NudgeBoard>,
    pub policy: SharedPolicy,
    pub diagnoser: Rc<FakeDiagnoser>,
    pub restore: Rc<FakeRestore>,
    pub advisor: Rc<FakeAdvisor>,
    pub compactions: HashMap<JobId, u32>,
    next_id: u64,
}

impl Rig {
    pub fn new(jev: bool) -> Rig {
        Rig::with(WatchdogSettings::default(), jev)
    }

    pub fn with(settings: WatchdogSettings, jev: bool) -> Rig {
        let wt = WorktreeRig::new();
        let runner = Rc::new(FakeRunner::default());
        let guard = GuardSettings::default();
        let jobs = JobService::new(
            wt.store.clone(),
            wt.store.clone(),
            runner.clone(),
            wt.clock.clone(),
        )
        .with_limits(Settings::default().limits())
        .with_loop_repeats(settings.breaker_repeats(guard.loop_repeats))
        .with_worktrees(wt.worktrees.clone())
        .with_event_tap();
        let board = NudgeBoard::new();
        let policy = SharedPolicy::new(guard.policy()).with_nudges(board.clone());
        let diagnoser = Rc::new(FakeDiagnoser::default());
        let restore = Rc::new(FakeRestore::default());
        let advisor = Rc::new(FakeAdvisor::default());
        let mut dog = Watchdog::new(
            settings,
            Rc::new(HookMessenger(board.clone())),
            wt.clock.clone(),
        )
        .with_ledger(wt.ledger_service.clone())
        .with_checkpoints(wt.checkpoints.clone())
        .with_restore(restore.clone())
        .with_diagnoser(diagnoser.clone());
        if jev {
            let consents = Rc::new(FakeConsents::default());
            let j = Jev::new(advisor.clone(), consents, wt.clock.clone(), 0.85).unwrap();
            j.grant().unwrap();
            dog = dog.with_jev(Rc::new(j));
        }
        Rig {
            wt,
            runner,
            jobs,
            dog,
            board,
            policy,
            diagnoser,
            restore,
            advisor,
            compactions: HashMap::new(),
            next_id: 0,
        }
    }

    pub fn now(&self) -> Millis {
        self.wt.clock.now()
    }

    pub fn wait(&mut self, ms: Millis) {
        self.wt.clock.advance(ms);
    }

    pub fn dispatch(&mut self, prompt: &str) -> JobId {
        let id = self.jobs.dispatch(self.wt.session.id, prompt).unwrap();
        self.pump();
        id
    }

    /// The runner index of `job`'s start.
    fn index_of(&self, job: JobId) -> usize {
        (0..self.runner.started())
            .rfind(|i| self.runner.request(*i).job_id == job)
            .expect("job started")
    }

    pub fn prompt_of(&self, job: JobId) -> String {
        self.runner.request(self.index_of(job)).prompt
    }

    pub fn model_of(&self, job: JobId) -> String {
        self.runner.request(self.index_of(job)).model
    }

    pub fn emit(&mut self, job: JobId, ev: AgentEvent) {
        let i = self.index_of(job);
        self.runner.emit(i, RunnerMsg::Event(ev));
    }

    /// The agent makes a call. Returns the watchdog's message when the call
    /// was not run because of it.
    pub fn call(
        &mut self,
        job: JobId,
        tool: &str,
        summary: &str,
        ok: bool,
        preview: &str,
    ) -> Option<String> {
        self.next_id += 1;
        let id = format!("t{}", self.next_id);
        let request = HookRequest {
            request_id: self.next_id,
            job_id: Some(job),
            tool_name: tool.into(),
            subject: summary.into(),
            input: String::new(),
            cwd: String::new(),
            content: String::new(),
        };
        let (decision, _) = self.policy.decide(&request);
        let nudge = (decision.verdict == Verdict::Deny)
            .then_some(decision.rule)
            .flatten()
            .filter(|r| r.starts_with(NUDGE_MARK));
        self.emit(
            job,
            AgentEvent::ToolUse {
                id: id.clone(),
                name: tool.into(),
                summary: summary.into(),
                actor: Actor::Lead,
            },
        );
        let (is_error, preview) = match &nudge {
            Some(text) => (true, format!("swarmux guard denied it: {text}")),
            None => (!ok, preview.to_string()),
        };
        self.emit(
            job,
            AgentEvent::ToolResult {
                tool_use_id: id,
                is_error,
                preview,
                actor: Actor::Lead,
            },
        );
        nudge
    }

    /// A call that starts and has not answered yet.
    pub fn start_call(&mut self, job: JobId, tool: &str, summary: &str) -> String {
        self.next_id += 1;
        let id = format!("t{}", self.next_id);
        self.emit(
            job,
            AgentEvent::ToolUse {
                id: id.clone(),
                name: tool.into(),
                summary: summary.into(),
                actor: Actor::Lead,
            },
        );
        id
    }

    pub fn end_call(&mut self, job: JobId, id: &str) {
        self.emit(
            job,
            AgentEvent::ToolResult {
                tool_use_id: id.into(),
                is_error: false,
                preview: "ok".into(),
                actor: Actor::Lead,
            },
        );
    }

    pub fn usage(&mut self, job: JobId, input: u64, output: u64) {
        self.emit(
            job,
            AgentEvent::Usage {
                input,
                output,
                cache_read: 0,
                cache_creation: 0,
                actor: Actor::Lead,
            },
        );
    }

    pub fn state(&mut self, job: JobId, state: SessionState) {
        self.emit(job, AgentEvent::SessionState(state));
    }

    pub fn finish(&mut self, job: JobId) {
        self.emit(
            job,
            AgentEvent::Result {
                is_error: false,
                subtype: "success".into(),
                cost_usd: 0.2,
                duration_ms: 1_000,
                num_turns: 3,
                tokens_in: 10,
                tokens_out: 10,
                cache_read: 0,
                cache_creation: None,
                context_window: None,
                subagents_spawned: 0,
                text: "done".into(),
            },
        );
    }

    pub fn pump(&mut self) -> WatchReport {
        let report = self.jobs.pump();
        let compactions = self.compactions.clone();
        let watch = self.dog.pump(&mut self.jobs, &report.events, &|j| {
            compactions.get(&j).copied().unwrap_or(0)
        });
        assert!(watch.errors.is_empty(), "{:?}", watch.errors);
        watch
    }

    pub fn status(&self, job: JobId) -> JobStatus {
        self.wt.store.status_of(job)
    }

    /// The job running now, newest first.
    pub fn running(&self) -> Option<JobId> {
        self.jobs
            .facts()
            .into_iter()
            .filter(|f| f.status == JobStatus::Running)
            .map(|f| f.job_id)
            .max()
    }

    /// Watchdog ledger entries as (event, rung).
    pub fn entries(&self) -> Vec<(String, String)> {
        self.wt
            .ledger
            .entries()
            .into_iter()
            .filter(|e| e.draft.kind == LedgerKind::Watchdog)
            .map(|e| {
                let p = &e.draft.payload_json;
                (field(p, "event"), field(p, "rung"))
            })
            .collect()
    }

    /// Whatever session runs keeps failing the same way, until the inbox.
    pub fn loop_until_escalated(&mut self) {
        for _ in 0..400 {
            if let Some(j) = self.running() {
                self.call(j, "Bash", "cargo test", false, ERR);
            }
            self.wait(10 * SEC);
            self.pump();
            if self.dog.stats().escalated > 0 {
                return;
            }
        }
        panic!("never escalated: {:?}", self.rungs());
    }

    pub fn rungs(&self) -> Vec<String> {
        self.entries()
            .into_iter()
            .filter(|(e, _)| e == "rung")
            .map(|(_, r)| r)
            .collect()
    }
}

/// A string field of a flat JSON object ("" when missing).
fn field(json: &str, key: &str) -> String {
    let tag = format!("\"{key}\":\"");
    json.find(&tag)
        .map(|at| {
            let rest = &json[at + tag.len()..];
            rest[..rest.find('"').unwrap_or(rest.len())].to_string()
        })
        .unwrap_or_default()
}

const ERR: &str = "error[E0425]: cannot find value `x` in this scope";

/// Two identical failures: the loop alarm and a nudge on the next try.
fn loop_twice(rig: &mut Rig, job: JobId) {
    rig.call(job, "Bash", "cargo test", false, ERR);
    rig.call(job, "Bash", "cargo test", false, ERR);
    rig.pump();
}

#[test]
fn a_loop_is_nudged_on_the_next_identical_call() {
    let mut rig = Rig::new(false);
    let job = rig.dispatch("fix the build");
    loop_twice(&mut rig, job);
    assert_eq!(rig.dog.health(job), Some(AlarmKind::Loop));
    assert_eq!(rig.rungs(), ["nudge"]);
    assert!(rig.board.is_waiting(job));
    // Another call is not the looping one: it runs.
    assert_eq!(rig.call(job, "Read", "src/lib.rs", true, "fn x"), None);
    let nudge = rig.call(job, "Bash", "cargo test", false, ERR).unwrap();
    assert!(
        nudge.starts_with("[swarmux watchdog] Bash `cargo test` failed the same way 2 times"),
        "{nudge}"
    );
    assert!(nudge.contains("try a different fix or approach"), "{nudge}");
    rig.pump();
    assert!(
        rig.entries()
            .contains(&("delivered".into(), "nudge".into()))
    );
    // The agent changes approach: the alarm clears and the job finishes.
    rig.call(job, "Edit", "src/lib.rs", true, "ok");
    rig.wait(5 * SEC);
    rig.pump();
    assert_eq!(rig.dog.health(job), None);
    rig.finish(job);
    rig.pump();
    assert_eq!(rig.dog.stats().rescued, 1);
    let events: Vec<String> = rig.entries().into_iter().map(|(e, _)| e).collect();
    assert_eq!(events, ["alarm", "rung", "delivered", "settled", "rescued"]);
}

#[test]
fn rungs_wait_for_their_cooldown() {
    let mut rig = Rig::new(false);
    let job = rig.dispatch("fix the build");
    loop_twice(&mut rig, job);
    rig.call(job, "Bash", "cargo test", false, ERR); // the nudge
    for _ in 0..3 {
        rig.wait(10 * SEC);
        rig.call(job, "Bash", "cargo test", false, ERR);
        rig.pump();
    }
    // Stopped by the loop breaker one failure after the nudge... and the
    // work goes on in a fresh session at once: an ended job needs no settling.
    assert_eq!(rig.rungs(), ["nudge", "fresh"]);
    assert_eq!(rig.dog.stats().taken(Rung::Nudge), 1, "no nudge spam");
}

#[test]
fn without_jev_the_judgement_is_skipped_for_a_fresh_session() {
    let mut rig = Rig::new(false);
    let job = rig.dispatch("fix the build\nsecond line");
    loop_twice(&mut rig, job);
    rig.wait(2 * MIN);
    // Still looping between other steps, after the nudge's cooldown.
    for _ in 0..3 {
        rig.call(job, "Edit", "src/lib.rs", true, "ok");
        rig.call(job, "Bash", "cargo test", false, ERR);
    }
    rig.pump();
    assert!(rig.advisor.requests().is_empty());
    assert_eq!(rig.rungs(), ["nudge", "fresh"]);
    assert_eq!(rig.status(job), JobStatus::Cancelled);
    let next = rig.running().unwrap();
    assert_ne!(next, job);
    let prompt = rig.prompt_of(next);
    assert!(
        prompt.starts_with("fix the build\nsecond line\n\n---\nHandoff from the previous session."),
        "{prompt}"
    );
    assert!(prompt.contains("Tried and failed:"), "{prompt}");
    // A recovery checkpoint holds the handoff; it moved on with the worktree.
    let cps = rig.wt.checkpoints.list(next).unwrap();
    assert_eq!(cps.last().unwrap().reason, CheckpointReason::Recovery);
    assert!(rig.wt.checkpoints.worktree(next).unwrap().is_some());
    assert!(!rig.board.is_waiting(job));
    assert_eq!(rig.dog.trail(next).len(), 2);
}

#[test]
fn jev_can_say_change_approach() {
    let mut rig = Rig::new(true);
    rig.advisor
        .answer_with(advice(crate::ports::AdviceAnswer::ChangeApproach, 0.93));
    let job = rig.dispatch("fix the build");
    loop_twice(&mut rig, job);
    rig.call(job, "Bash", "cargo test", false, ERR); // nudged
    rig.wait(100 * SEC);
    rig.call(job, "Bash", "cargo test", false, ERR);
    rig.call(job, "Bash", "cargo test", false, ERR);
    rig.pump();
    rig.pump();
    assert_eq!(rig.rungs(), ["nudge", "judge"]);
    let AdviceRequest::Recovery { situation, tried } = &rig.advisor.requests()[0] else {
        panic!()
    };
    assert!(
        situation.starts_with("loop: Bash `cargo test`"),
        "{situation}"
    );
    assert_eq!(tried.len(), 1);
    let nudge = rig.call(job, "Read", "x", true, "").unwrap();
    assert!(nudge.contains("independent check"), "{nudge}");
    assert!(
        rig.dog
            .trail(job)
            .iter()
            .any(|t| t.starts_with("JEV: change approach (0.93)"))
    );
}

#[test]
fn jev_saying_carry_on_stands_the_watchdog_down() {
    let mut rig = Rig::new(true);
    rig.advisor
        .answer_with(advice(crate::ports::AdviceAnswer::Continue, 0.9));
    let job = rig.dispatch("long refactor");
    // Repeats earlier calls for a while: looks like a stall.
    rig.call(job, "Read", "a.rs", true, "");
    rig.call(job, "Read", "b.rs", true, "");
    for i in 0..15u32 {
        let f = if i.is_multiple_of(2) { "a.rs" } else { "b.rs" };
        rig.call(job, "Read", f, true, "");
    }
    rig.pump();
    assert_eq!(rig.rungs(), ["nudge"]);
    rig.wait(2 * MIN);
    rig.pump();
    assert_eq!(rig.rungs(), ["nudge", "judge"]);
    rig.pump();
    assert_eq!(rig.dog.health(job), None);
    rig.wait(MIN);
    rig.pump();
    assert_eq!(rig.rungs(), ["nudge", "judge"], "stood down");
}

#[test]
fn an_unsure_jev_changes_nothing() {
    let mut rig = Rig::new(true);
    rig.advisor
        .answer_with(advice(crate::ports::AdviceAnswer::Stop, 0.5));
    let job = rig.dispatch("x");
    loop_twice(&mut rig, job);
    rig.call(job, "Bash", "cargo test", false, ERR); // the nudge
    rig.wait(2 * MIN);
    rig.call(job, "Bash", "cargo test", false, ERR);
    rig.call(job, "Bash", "cargo test", false, ERR);
    rig.pump();
    rig.pump();
    assert!(
        rig.dog
            .trail(job)
            .iter()
            .any(|t| t == "JEV unsure (0.50): nothing changed")
    );
    assert_eq!(rig.status(job), JobStatus::Running);
}

#[test]
fn a_silent_stall_is_found_within_three_minutes_and_skips_the_nudge() {
    let mut rig = Rig::new(false);
    let job = rig.dispatch("x");
    rig.call(job, "Read", "a.rs", true, "");
    rig.pump();
    let quiet_from = rig.now();
    let mut found = None;
    for _ in 0..60 {
        rig.wait(5 * SEC);
        rig.pump();
        if rig.dog.health(job).is_some() || rig.status(job) != JobStatus::Running {
            found = Some(rig.now() - quiet_from);
            break;
        }
    }
    let ttd = found.expect("stall detected");
    assert!(ttd <= 3 * MIN, "detected after {ttd} ms");
    assert_eq!(rig.rungs(), ["fresh"], "a silent agent can't be nudged");
}

#[test]
fn context_pressure_goes_straight_to_a_fresh_session() {
    let mut rig = Rig::new(true);
    let job = rig.dispatch("x");
    rig.usage(job, 900_000, 10);
    rig.pump();
    assert_eq!(rig.rungs(), ["fresh"]);
    assert!(rig.advisor.requests().is_empty());
}

#[test]
fn compactions_count_as_pressure() {
    let mut rig = Rig::new(false);
    let job = rig.dispatch("x");
    rig.compactions.insert(job, 2);
    rig.pump();
    assert_eq!(rig.rungs(), ["fresh"]);
}

#[test]
fn diagnosis_then_fork_from_the_checkpoint_before_the_trouble() {
    let mut rig = Rig::new(false);
    rig.diagnoser.answer_with(diagnosis(
        "the test edits the fixture",
        "Leave tests/fixture.json alone.",
    ));
    let job = rig.dispatch("fix the parser");
    // A good point, before any trouble.
    rig.call(job, "Edit", "src/parse.rs", true, "ok");
    let good = rig
        .wt
        .checkpoints
        .take(job, "", CheckpointReason::GatePass, &Default::default())
        .unwrap()
        .unwrap();
    rig.wait(MIN);
    rig.pump();
    // Loops in every session until the inbox.
    rig.loop_until_escalated();
    assert_eq!(
        rig.rungs(),
        ["nudge", "fresh", "diagnose", "fork", "escalate", "inbox"]
    );
    let req = &rig.diagnoser.requests()[0];
    assert!(
        req.steps.iter().any(|s| s.contains("cargo test")),
        "{:?}",
        req.steps
    );
    assert_eq!(rig.restore.calls.borrow()[0].1, good.commit);
    let forked = rig
        .jobs
        .facts()
        .into_iter()
        .find(|f| f.prompt.contains("were put back"))
        .expect("forked job");
    assert!(
        forked.prompt.contains("Leave tests/fixture.json alone."),
        "{}",
        forked.prompt
    );
    // Escalation moved the work up to opus.
    assert!(rig.jobs.facts().iter().any(|f| f.model == "opus"));
    let items = rig.dog.escalations();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, AttentionKind::Recovery);
    assert!(
        items[0].detail.contains("tried nudge → fresh session"),
        "{}",
        items[0].detail
    );
    assert!(
        items[0]
            .detail
            .contains("diagnosis: the test edits the fixture")
    );
    // Three restarts, the default cap: replan was not reached.
    assert_eq!(rig.dog.stats().escalated, 1);
}

#[test]
fn replan_when_there_is_nothing_stronger() {
    let settings = WatchdogSettings {
        escalate_to: vec!["sonnet".into()],
        ..WatchdogSettings::default()
    };
    let mut rig = Rig::with(settings, false);
    rig.dispatch("x");
    rig.loop_until_escalated();
    assert_eq!(
        rig.rungs(),
        ["nudge", "fresh", "diagnose", "replan", "inbox"]
    );
    assert!(
        rig.jobs
            .facts()
            .iter()
            .any(|f| f.prompt.contains("numbered plan"))
    );
}

#[test]
fn waiting_for_the_user_is_left_alone() {
    let mut rig = Rig::new(false);
    let job = rig.dispatch("x");
    rig.state(job, SessionState::RequiresAction);
    for _ in 0..20 {
        rig.wait(30 * SEC);
        rig.pump();
    }
    assert!(rig.entries().is_empty());
}

#[test]
fn turned_off_it_does_nothing_and_the_loop_breaker_stops_as_before() {
    let settings = WatchdogSettings {
        enabled: false,
        ..WatchdogSettings::default()
    };
    let mut rig = Rig::with(settings, false);
    let job = rig.dispatch("x");
    loop_twice(&mut rig, job);
    assert!(rig.entries().is_empty());
    assert_eq!(rig.status(job), JobStatus::Cancelled);
}

#[test]
fn a_job_that_simply_fails_is_not_retried() {
    let mut rig = Rig::new(false);
    let job = rig.dispatch("x");
    rig.call(job, "Read", "a", true, "");
    rig.emit(
        job,
        AgentEvent::Result {
            is_error: true,
            subtype: "error_during_execution".into(),
            cost_usd: 0.1,
            duration_ms: 1,
            num_turns: 1,
            tokens_in: 1,
            tokens_out: 1,
            cache_read: 0,
            cache_creation: None,
            context_window: None,
            subagents_spawned: 0,
            text: "broke".into(),
        },
    );
    rig.pump();
    assert!(rig.entries().is_empty());
    assert_eq!(rig.runner.started(), 1);
}

#[test]
fn the_board_matches_cut_summaries_and_any_call() {
    let board = NudgeBoard::new();
    let messenger = HookMessenger(board.clone());
    messenger.send(
        JobId(1),
        Nudge {
            text: "hi".into(),
            call: Some(("Bash".into(), "cargo test --workspace -- --nocap…".into())),
        },
    );
    assert_eq!(board.take(JobId(2), "Bash", "cargo test --workspace"), None);
    assert_eq!(board.take(JobId(1), "Read", "x"), None);
    assert_eq!(
        board.take(JobId(1), "Bash", "cargo test --workspace -- --nocapture"),
        Some("[swarmux watchdog] hi".into())
    );
    assert_eq!(messenger.delivered(), [(JobId(1), "hi".to_string())]);
    messenger.send(
        JobId(1),
        Nudge {
            text: "any".into(),
            call: None,
        },
    );
    messenger.forget(JobId(1));
    assert_eq!(board.take(JobId(1), "Read", "x"), None);
    assert!(!NoMessenger.send(
        JobId(1),
        Nudge {
            text: String::new(),
            call: None
        }
    ));
}

#[test]
fn stronger_models_follow_the_list_then_the_family() {
    let rig = Rig::new(false);
    let up = |m: &str| rig.dog.stronger(m);
    assert_eq!(up("sonnet").as_deref(), Some("opus"));
    assert_eq!(up("claude-sonnet-4-5").as_deref(), Some("opus"));
    assert_eq!(up("opus"), None);
    assert_eq!(up("haiku").as_deref(), Some("sonnet"));
    assert_eq!(up("fable"), None, "never down");
    assert_eq!(up("some-local-model"), None);
}

#[test]
fn breaker_runs_one_failure_behind_the_watchdog() {
    let on = WatchdogSettings::default();
    assert_eq!(on.breaker_repeats(2), 3);
    assert_eq!(on.breaker_repeats(0), 0);
    let off = WatchdogSettings {
        enabled: false,
        ..WatchdogSettings::default()
    };
    assert_eq!(off.breaker_repeats(2), 2);
}
