//! Planted failures: scripted agents that get stuck in known ways, each
//! cured by one known kind of help, next to healthy agents that only look
//! odd (long builds, polling, slow thinking). The suite runs them through
//! the real job service and watchdog on fakes and checks the targets:
//!
//! - a silent stall is detected within 3 minutes,
//! - at least 25% of planted failures are rescued below the inbox rung,
//! - at most 10% of nudges land on healthy agents.
//!
//! The agents' responses are planted, so this measures the watchdog's
//! mechanics (detect, climb, cool down, hand over), not how real agents
//! respond. Run with `--nocapture` to see the table.

use std::collections::HashMap;

use swarmux_domain::{CheckpointReason, JobId, JobStatus, Millis, SessionState};

use super::tests::{MIN, Rig, SEC};
use crate::testing::diagnosis;

const TICK: Millis = 5 * SEC;
/// The longest a case may run, in simulated time.
const MAX_TICKS: usize = 4 * 60 * 12;
const ERR: &str = "error[E0599]: no method named `parse_all` found for struct `Parser`";

/// How a planted agent gets stuck.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Trouble {
    /// The same failing command, again and again.
    Loop,
    /// Edit, test, same error, edit, test, same error.
    Oscillation,
    /// Rereads the same two files without changing anything.
    Spin,
    /// Hangs: no events at all.
    Silent,
    /// The context is nearly full.
    Context,
    /// Spends far too fast.
    Burn,
}

/// What gets a planted agent unstuck.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cure {
    /// Being told, on its next call.
    Nudge,
    /// A new session with a handoff.
    Fresh,
    /// The diagnosis, delivered.
    Diagnosis,
    /// Going back to the checkpoint before the trouble.
    Fork,
    /// A stronger model.
    Stronger,
    /// Nothing: only the user can.
    Never,
}

struct Case {
    name: &'static str,
    trouble: Trouble,
    cure: Cure,
    /// A checkpoint is taken before the trouble starts.
    checkpoint: bool,
}

const fn case(name: &'static str, trouble: Trouble, cure: Cure, checkpoint: bool) -> Case {
    Case {
        name,
        trouble,
        cure,
        checkpoint,
    }
}

const FAILURES: [Case; 14] = [
    case("loop, heeds a nudge", Trouble::Loop, Cure::Nudge, false),
    case(
        "oscillation, heeds a nudge",
        Trouble::Oscillation,
        Cure::Nudge,
        false,
    ),
    case("spin, heeds a nudge", Trouble::Spin, Cure::Nudge, false),
    case("burn, heeds a nudge", Trouble::Burn, Cure::Nudge, false),
    case(
        "loop, needs a fresh session",
        Trouble::Loop,
        Cure::Fresh,
        false,
    ),
    case(
        "hang, needs a fresh session",
        Trouble::Silent,
        Cure::Fresh,
        false,
    ),
    case(
        "full context, needs a fresh session",
        Trouble::Context,
        Cure::Fresh,
        false,
    ),
    case(
        "oscillation, needs a fresh session",
        Trouble::Oscillation,
        Cure::Fresh,
        false,
    ),
    case(
        "loop, needs the diagnosis",
        Trouble::Loop,
        Cure::Diagnosis,
        false,
    ),
    case(
        "loop, needs the checkpoint",
        Trouble::Loop,
        Cure::Fork,
        true,
    ),
    case(
        "loop, needs a stronger model",
        Trouble::Loop,
        Cure::Stronger,
        false,
    ),
    case("loop, hopeless", Trouble::Loop, Cure::Never, true),
    case("hang, hopeless", Trouble::Silent, Cure::Never, false),
    case("spin, hopeless", Trouble::Spin, Cure::Never, false),
];

/// One tick of a scripted agent.
#[derive(Clone, Debug)]
enum Act {
    Call(&'static str, String, bool, &'static str),
    /// A call that runs until [`Act::End`].
    Start(&'static str, &'static str),
    End,
    Usage(u64, u64),
    State(SessionState),
    Quiet,
    Finish,
}

fn edit(i: usize) -> Act {
    Act::Call("Edit", format!("src/m{i}.rs"), true, "ok")
}

fn read(i: usize) -> Act {
    Act::Call("Read", format!("src/f{i}.rs"), true, "fn main() {}")
}

/// Healthy agents: odd-looking but fine.
fn healthy() -> Vec<(&'static str, Vec<Act>)> {
    let finish = |mut v: Vec<Act>| {
        v.extend((0..3).map(edit));
        v.push(Act::Finish);
        v
    };
    let mut out = Vec::new();
    // A release build that takes six minutes.
    let mut build = vec![Act::Start("Bash", "cargo build --release")];
    build.extend((0..72).map(|_| Act::Quiet));
    build.push(Act::End);
    out.push(("long build", finish(build)));
    // Reads thirty files before touching any.
    out.push(("exploration", finish((0..30).map(read).collect())));
    // Test-driven: a different failure each round, then green.
    let mut tdd = Vec::new();
    for k in 0..6 {
        tdd.push(edit(10 + k));
        tdd.push(Act::Call(
            "Bash",
            "cargo test".into(),
            false,
            [
                "assertion failed: a",
                "assertion failed: b",
                "panicked at c",
                "error: d",
                "mismatch e",
                "error: f",
            ][k],
        ));
    }
    tdd.push(Act::Call("Bash", "cargo test".into(), true, "ok"));
    out.push(("test-driven", finish(tdd)));
    // Waits for CI: the same successful call every half minute.
    let mut poll = Vec::new();
    for _ in 0..12 {
        poll.push(Act::Call(
            "Bash",
            "gh run view 123".into(),
            true,
            "in progress",
        ));
        poll.extend((0..5).map(|_| Act::Quiet));
    }
    out.push(("polls CI", finish(poll)));
    // Thinks for a minute and a half between calls.
    let mut think = Vec::new();
    for i in 0..5 {
        think.push(read(i));
        think.extend((0..18).map(|_| Act::Quiet));
    }
    out.push(("slow thinker", finish(think)));
    // One transient failure, retried once.
    out.push((
        "transient retry",
        finish(vec![
            Act::Call(
                "Bash",
                "npm install".into(),
                false,
                "npm ERR! network ETIMEDOUT",
            ),
            Act::Call("Bash", "npm install".into(), true, "added 3 packages"),
        ]),
    ));
    // Waits five minutes for the user.
    let mut wait = vec![Act::State(SessionState::RequiresAction)];
    wait.extend((0..60).map(|_| Act::Quiet));
    wait.push(Act::State(SessionState::Running));
    out.push(("waits for the user", finish(wait)));
    // Spends steadily, well under the limit.
    let mut spend = Vec::new();
    for i in 0..40 {
        spend.push(Act::Usage(20_000, 2_000));
        spend.push(read(100 + i));
    }
    out.push(("steady spender", finish(spend)));
    // Uses most of the context, but not all.
    let mut big = vec![Act::Usage(600_000, 1_000)];
    big.extend((0..10).map(read));
    out.push(("big context", finish(big)));
    // A large refactor: many files, tests green.
    let mut refactor: Vec<Act> = (0..20).map(|i| edit(200 + i)).collect();
    refactor.push(Act::Call("Bash", "cargo test".into(), true, "ok"));
    out.push(("large refactor", finish(refactor)));
    out
}

/// What one run of a case came to.
#[derive(Debug, Default)]
struct Outcome {
    rescued: bool,
    escalated: bool,
    nudges: u32,
    rungs: Vec<String>,
    /// From the moment an agent went quiet to the first alarm.
    detected_after: Option<Millis>,
}

/// What a planted agent has been told so far.
#[derive(Default)]
struct Heard {
    nudged: bool,
    diagnosed: bool,
    step: usize,
    cured_steps: Option<usize>,
}

fn is_cured(rig: &Rig, job: JobId, cure: Cure, heard: &Heard) -> bool {
    let prompt = rig.prompt_of(job);
    match cure {
        Cure::Nudge => heard.nudged,
        // Told on a call, or handed over in a new session's briefing.
        Cure::Diagnosis => heard.diagnosed || prompt.contains("Add the method before"),
        Cure::Fresh => prompt.contains("Handoff from the previous session"),
        Cure::Fork => prompt.contains("were put back"),
        Cure::Stronger => rig.model_of(job) == "opus",
        Cure::Never => false,
    }
}

fn stuck_act(trouble: Trouble, step: usize) -> Act {
    match trouble {
        Trouble::Loop => Act::Call("Bash", "cargo test".into(), false, ERR),
        Trouble::Oscillation if step.is_multiple_of(2) => {
            Act::Call("Edit", "src/parse.rs".into(), true, "ok")
        }
        Trouble::Oscillation => Act::Call("Bash", "cargo test".into(), false, ERR),
        Trouble::Spin => Act::Call(
            "Read",
            if step.is_multiple_of(2) {
                "src/a.rs"
            } else {
                "src/b.rs"
            }
            .into(),
            true,
            "fn a() {}",
        ),
        Trouble::Silent => Act::Quiet,
        Trouble::Context if step == 0 => Act::Usage(900_000, 100),
        Trouble::Burn if step.is_multiple_of(2) => Act::Usage(0, 200_000),
        Trouble::Context | Trouble::Burn => read(1_000 + step),
    }
}

/// Plays one act. Returns false when the agent finished.
fn play(rig: &mut Rig, job: JobId, act: Act, heard: &mut Heard) -> bool {
    match act {
        Act::Call(tool, summary, ok, preview) => {
            if let Some(text) = rig.call(job, tool, &summary, ok, preview) {
                heard.nudged = true;
                heard.diagnosed |= text.contains("diagnosis");
            }
        }
        Act::Start(tool, summary) => {
            let id = rig.start_call(job, tool, summary);
            OPEN.with(|o| o.borrow_mut().insert(job, id));
        }
        Act::End => {
            if let Some(id) = OPEN.with(|o| o.borrow_mut().remove(&job)) {
                rig.end_call(job, &id);
            }
        }
        Act::Usage(input, output) => rig.usage(job, input, output),
        Act::State(s) => rig.state(job, s),
        Act::Quiet => {}
        Act::Finish => {
            rig.finish(job);
            return false;
        }
    }
    true
}

thread_local! {
    static OPEN: std::cell::RefCell<HashMap<JobId, String>> = std::cell::RefCell::new(HashMap::new());
}

/// Runs one agent script (healthy) or planted trouble (stuck) to its end.
fn run(case: Option<&Case>, script: &[Act]) -> Outcome {
    let mut rig = Rig::new();
    rig.diagnoser.answer_with(diagnosis(
        "the same command keeps failing on a missing method",
        "Add the method before running the tests again.",
    ));
    let first = rig.dispatch("make the parser handle every input");
    if case.is_some_and(|c| c.checkpoint) {
        rig.call(first, "Edit", "src/parse.rs", true, "ok");
        rig.wt
            .checkpoints
            .take(first, "", CheckpointReason::GatePass, &Default::default())
            .unwrap();
        rig.wait(MIN);
        rig.pump();
    }
    let mut heard: HashMap<JobId, Heard> = HashMap::new();
    let mut out = Outcome::default();
    let mut quiet_since: Option<Millis> = None;
    for _ in 0..MAX_TICKS {
        if let Some(job) = rig.running() {
            let h = heard.entry(job).or_default();
            let act = match case {
                None => script.get(h.step).cloned().unwrap_or(Act::Finish),
                Some(c) => {
                    if h.cured_steps.is_none() && is_cured(&rig, job, c.cure, h) {
                        h.cured_steps = Some(0);
                    }
                    match h.cured_steps {
                        Some(n) if n >= 3 => Act::Finish,
                        Some(n) => edit(500 + n),
                        None => stuck_act(c.trouble, h.step),
                    }
                }
            };
            if matches!(act, Act::Quiet) {
                quiet_since.get_or_insert(rig.now());
            } else {
                quiet_since = None;
            }
            let h = heard.get_mut(&job).unwrap();
            h.step += 1;
            if let Some(n) = h.cured_steps.as_mut() {
                *n += 1;
            }
            play(&mut rig, job, act, h);
        }
        rig.wait(TICK);
        rig.pump();
        if out.detected_after.is_none()
            && rig.entries().iter().any(|(e, _)| e == "alarm")
            && let Some(q) = quiet_since
        {
            out.detected_after = Some(rig.now() - q);
        }
        let done = rig.jobs.facts().iter().any(|f| f.status == JobStatus::Done);
        if rig.running().is_none() && (done || rig.dog.stats().escalated > 0) {
            break;
        }
    }
    let stats = rig.dog.stats();
    out.rescued = stats.rescued > 0;
    out.escalated = stats.escalated > 0;
    out.nudges = stats.taken(swarmux_domain::Rung::Nudge);
    out.rungs = rig.rungs();
    out
}

fn suite() {
    let mut rescued = 0;
    let mut nudges = 0;
    let mut false_nudges = 0;
    let mut false_rungs = 0;
    eprintln!("planted failures:");
    for c in &FAILURES {
        let o = run(Some(c), &[]);
        rescued += u32::from(o.rescued);
        nudges += o.nudges;
        let how = if o.rescued {
            "rescued"
        } else if o.escalated {
            "inbox"
        } else {
            "lost"
        };
        eprintln!("  {:<38} {how:<8} {}", c.name, o.rungs.join(" → "));
        assert!(
            o.rescued || o.escalated,
            "{}: neither rescued nor in the inbox",
            c.name
        );
        assert!(o.nudges <= 1, "{}: nudged {} times", c.name, o.nudges);
        if c.cure == Cure::Never {
            assert!(o.escalated && !o.rescued, "{}", c.name);
        }
        if c.trouble == Trouble::Silent {
            let after = o.detected_after.expect("stall detected");
            eprintln!("    stall detected after {} s", after / SEC);
            assert!(
                after <= 3 * MIN,
                "{}: stall detected after {after} ms",
                c.name
            );
        }
    }
    for (name, script) in healthy() {
        let o = run(None, &script);
        false_nudges += o.nudges;
        false_rungs += o.rungs.len();
        eprintln!(
            "  healthy: {name:<29} {}",
            if o.rungs.is_empty() {
                "left alone".to_string()
            } else {
                o.rungs.join(" → ")
            }
        );
    }
    let total = FAILURES.len() as f64;
    let rate = f64::from(rescued) / total;
    let all_nudges = nudges + false_nudges;
    let false_rate = if all_nudges == 0 {
        0.0
    } else {
        f64::from(false_nudges) / f64::from(all_nudges)
    };
    eprintln!(
        "  rescued below the inbox: {rescued}/{} ({:.0}%) · false nudges: {false_nudges}/{all_nudges} ({:.0}%) · other rungs on healthy agents: {false_rungs}",
        FAILURES.len(),
        rate * 100.0,
        false_rate * 100.0
    );
    assert!(rate >= 0.25, "rescued {rate}");
    assert!(false_rate <= 0.10, "false nudges {false_rate}");
}

#[test]
fn planted_failures() {
    suite();
}
