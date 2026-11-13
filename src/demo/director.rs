//! Plays the script: once per update it moves the demo's clock and carries
//! out the steps that are due, by the same doors real work comes in
//! through (a job's output, a tool call at the Guard, a stop at the gate).

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use swarmux_application::hookbus::{StopContext, StopPolicy};
use swarmux_application::ports::{HookRequest, RunnerMsg};
use swarmux_application::{GateKeeper, SharedPolicy};
use swarmux_domain::telemetry::RateLimit;
use swarmux_domain::{Actor, AgentEvent, HookEvent, JobId};
use swarmux_infrastructure::demo::{DemoChecker, DemoClock, ScriptedInbox, ScriptedRunner};
use swarmux_tui::App;

use super::script::{self, Act, CLOCK_MS_PER_TICK, START};

/// What deciding a call took, in microseconds, as the Guard logs it.
const DECIDED_IN_US: u64 = 40;

/// What the director works with.
pub struct Parts {
    /// The throwaway folder; it goes when the demo ends.
    pub _dir: tempfile::TempDir,
    pub clock: Rc<DemoClock>,
    pub runner: Rc<ScriptedRunner>,
    pub inbox: Rc<ScriptedInbox>,
    pub keeper: Arc<GateKeeper>,
    pub policy: SharedPolicy,
    pub checker: Arc<DemoChecker>,
    /// Each session's folder.
    pub cwds: Vec<(&'static str, PathBuf)>,
    /// The mission's slug.
    pub mission: String,
    pub seed: u64,
}

pub struct Director {
    parts: Parts,
    steps: Vec<(u64, Act)>,
    next: usize,
    tick: u64,
    jobs: HashMap<&'static str, JobId>,
    rng: u64,
    ids: u64,
}

impl Director {
    pub fn new(parts: Parts) -> Director {
        let steps = script::timeline(&parts.mission);
        // The mission's checks fail the first time the gate runs them (and
        // on the retry it makes), so its first "done" is sent back.
        if let Some((_, root)) = parts.cwds.first() {
            parts.checker.fail(root, 2);
        }
        Director {
            rng: parts.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            parts,
            steps,
            next: 0,
            tick: 0,
            jobs: HashMap::new(),
            ids: 0,
        }
    }

    /// The function the app calls at the start of every update.
    pub fn into_driver(mut self) -> impl FnMut(&mut App) {
        move |app| self.step(app)
    }

    fn rand(&mut self, low: u64, high: u64) -> u64 {
        // xorshift64
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        low + self.rng % (high - low + 1)
    }

    fn step(&mut self, app: &mut App) {
        let now = START + i64::try_from(self.tick).unwrap_or(0) * CLOCK_MS_PER_TICK;
        self.parts.clock.set(now);
        while let Some((due, _)) = self.steps.get(self.next) {
            if *due > self.tick {
                break;
            }
            let act = self.steps[self.next].1.clone();
            if !self.perform(app, &act) {
                // Not ready yet (a job that has not started): try next update.
                break;
            }
            self.next += 1;
        }
        self.tick += 1;
    }

    fn job(&self, label: &str) -> Option<JobId> {
        self.jobs
            .get(label)
            .copied()
            .filter(|j| self.parts.runner.is_started(*j))
    }

    fn emit(&self, label: &str, event: AgentEvent) -> bool {
        match self.job(label) {
            Some(job) => self.parts.runner.emit(job, RunnerMsg::Event(event)),
            None => false,
        }
    }

    fn cwd(&self, label: &str) -> PathBuf {
        self.parts
            .cwds
            .iter()
            .find(|(name, _)| *name == label)
            .map(|(_, p)| p.clone())
            .unwrap_or_default()
    }

    fn perform(&mut self, app: &mut App, act: &Act) -> bool {
        match act {
            Act::Dispatch {
                label,
                session,
                prompt,
            } => {
                if let Ok(job) = app.dispatch_to(session, prompt) {
                    self.jobs.insert(label, job);
                }
                true
            }
            Act::Mission { label, slug } => {
                if let Ok(job) = app.run_mission(slug) {
                    self.jobs.insert(label, job);
                }
                true
            }
            Act::Init(label) => self.emit(
                label,
                AgentEvent::Init {
                    session_id: format!("demo-{label}"),
                    model: "sonnet".into(),
                    cwd: self.cwd(label).display().to_string(),
                    permission_mode: "default".into(),
                },
            ),
            Act::Text(label, text) => self.emit(
                label,
                AgentEvent::Text {
                    text: (*text).to_string(),
                    actor: Actor::Lead,
                },
            ),
            Act::Tool {
                label,
                name,
                summary,
                ok,
                preview,
            } => {
                self.ids += 1;
                let id = format!("toolu_demo_{}", self.ids);
                self.emit(
                    label,
                    AgentEvent::ToolUse {
                        id: id.clone(),
                        name: (*name).to_string(),
                        summary: (*summary).to_string(),
                        actor: Actor::Lead,
                    },
                ) && self.emit(
                    label,
                    AgentEvent::ToolResult {
                        tool_use_id: id,
                        is_error: !ok,
                        preview: (*preview).to_string(),
                        actor: Actor::Lead,
                    },
                )
            }
            Act::Usage(label) => {
                let event = AgentEvent::Usage {
                    input: self.rand(1_200, 4_500),
                    output: self.rand(300, 1_400),
                    cache_read: self.rand(8_000, 30_000),
                    cache_creation: self.rand(500, 2_000),
                    actor: Actor::Lead,
                };
                self.emit(label, event)
            }
            Act::Limit {
                label,
                five_hour,
                seven_day,
            } => {
                let limit = |window: &str, used: f64, resets_in_ms: i64| RateLimit {
                    window: window.to_string(),
                    status: "allowed".into(),
                    utilization: Some(used),
                    resets_at: Some(START + resets_in_ms),
                    surpassed_threshold: None,
                };
                let hour = 3_600_000;
                self.emit(
                    label,
                    AgentEvent::RateLimit(limit("five_hour", *five_hour, 3 * hour)),
                ) && self.emit(
                    label,
                    AgentEvent::RateLimit(limit("seven_day", *seven_day, 74 * hour)),
                )
            }
            Act::Call {
                label,
                tool,
                subject,
            } => {
                let Some(job) = self.job(label) else {
                    return false;
                };
                let cwd = self.cwd(label).display().to_string().replace('\\', "/");
                let subject = (*subject).to_string();
                let field = if *tool == "Bash" {
                    "command"
                } else {
                    "file_path"
                };
                self.ids += 1;
                let request = HookRequest {
                    request_id: self.ids,
                    job_id: Some(job),
                    tool_name: (*tool).to_string(),
                    input: format!("{{\"{field}\":\"{subject}\"}}"),
                    subject,
                    cwd,
                    content: String::new(),
                    ..HookRequest::default()
                };
                // The transport answers from the policy; a call it asks about
                // waits for the user. The time deciding took is fixed: the
                // ledger records it, and runs must not differ.
                let answered = Some((self.parts.policy.decide(&request).0, DECIDED_IN_US));
                self.parts.inbox.push(request, answered);
                true
            }
            Act::Stop { label, message } => {
                let Some(job) = self.job(label) else {
                    return false;
                };
                let answer = self.parts.keeper.on_stop(&StopContext {
                    event: HookEvent::Stop,
                    job: Some(job),
                    session: format!("demo-{label}"),
                    cwd: self.cwd(label).display().to_string(),
                    last_message: (*message).to_string(),
                });
                let _ = answer;
                true
            }
            Act::Finish {
                label,
                cost_usd,
                turns,
                minutes,
                text,
            } => {
                let Some(job) = self.job(label) else {
                    return false;
                };
                let tokens_in = self.rand(90_000, 240_000);
                let tokens_out = self.rand(14_000, 38_000);
                let result = AgentEvent::Result {
                    is_error: false,
                    subtype: "success".into(),
                    cost_usd: *cost_usd,
                    duration_ms: minutes * 60_000,
                    num_turns: *turns,
                    tokens_in,
                    tokens_out,
                    cache_read: tokens_in * 3,
                    cache_creation: None,
                    context_window: Some(200_000),
                    subagents_spawned: 0,
                    text: (*text).to_string(),
                };
                self.parts.runner.emit(job, RunnerMsg::Event(result));
                self.parts.runner.emit(
                    job,
                    RunnerMsg::Exited {
                        code: Some(0),
                        stderr_tail: String::new(),
                    },
                );
                true
            }
        }
    }
}
