//! The engine: the services that keep the work going whether anyone looks
//! or not — jobs, the Guard, the agents' hooks, the done-gate, the
//! watchdog, missions and their task graphs, and the board — and the order
//! they are pumped in, since each reads what the one before it took in.
//!
//! The terminal interface holds one and pumps it every frame; in the
//! background service that interface runs with no terminal of its own,
//! and the engine with it.

use std::collections::HashMap;
use std::rc::Rc;

use swarmux_domain::JobId;

use crate::board::{BoardInbox, BoardService};
use crate::gate::{GateService, GateState};
use crate::guard::GuardService;
use crate::hookbus::HookBus;
use crate::jobs::{JobFact, JobService};
use crate::knowledge::KnowledgeService;
use crate::missions::MissionService;
use crate::watchdog::Watchdog;
use crate::{AppError, AppResult};

/// The engine's services. They are public: use cases are called on them
/// directly, and only the cross-service steps live here.
pub struct Engine {
    pub jobs: JobService,
    pub guard: GuardService,
    /// Lifecycle events the agents' hooks report.
    pub hooks: Option<HookBus>,
    /// The done-gate's verdicts.
    pub gate: Option<GateService>,
    /// Notices stuck agents and tries to get them going again.
    pub watchdog: Option<Watchdog>,
    /// Writes, lints and follows missions.
    pub missions: Option<MissionService>,
    /// The mission boards and where agents' calls arrive.
    pub board: Option<(Rc<BoardService>, Rc<dyn BoardInbox>)>,
    /// Rules, facts and decisions, and what reached the agents.
    pub knowledge: Option<Rc<KnowledgeService>>,
}

/// What one [`Engine::pump`] did.
#[derive(Debug, Default)]
pub struct EngineReport {
    pub errors: Vec<AppError>,
    /// A job made progress, started or ended.
    pub jobs_changed: bool,
    /// Sessions were made (forks, the watchdog's fresh sessions) or used,
    /// so the session list re-sorts.
    pub sessions_changed: bool,
}

/// What one [`Engine::pump_missions`] did.
#[derive(Debug, Default)]
pub struct MissionsPump {
    pub errors: Vec<AppError>,
    /// Task graphs started jobs, in sessions of their own.
    pub graphs_changed: bool,
}

impl Engine {
    pub fn new(jobs: JobService, guard: GuardService) -> Engine {
        Engine {
            jobs,
            guard,
            hooks: None,
            gate: None,
            watchdog: None,
            missions: None,
            board: None,
            knowledge: None,
        }
    }

    /// Gives the watchdog the job events it watches.
    pub fn set_watchdog(&mut self, watchdog: Watchdog) {
        self.jobs.set_event_tap(watchdog.settings().enabled);
        self.watchdog = Some(watchdog);
    }

    /// Applies what the runners, the hooks and the gate reported, lets the
    /// watchdog act on stuck agents and answers the agents' board calls.
    pub fn pump(&mut self) -> EngineReport {
        let report = self.jobs.pump();
        let mut errors = report.errors;
        for (job, trip) in &report.loop_trips {
            errors.extend(self.guard.record_loop(*job, trip).err());
        }
        let mut watched = false;
        if let Some(dog) = &mut self.watchdog {
            let hooks = &self.hooks;
            let compactions = |job| {
                hooks
                    .as_ref()
                    .and_then(|h| h.signals(job))
                    .map_or(0, |s| s.compactions)
            };
            let w = dog.pump(&mut self.jobs, &report.events, &compactions);
            errors.extend(w.errors);
            watched = w.changed || !w.new_sessions.is_empty();
        }
        let guard = self.guard.pump();
        errors.extend(guard.errors);
        for job in guard.decisions.iter().filter_map(|d| d.job_id) {
            self.jobs.observe_decision(job);
        }
        if let Some(hooks) = &mut self.hooks {
            errors.extend(hooks.pump());
        }
        if let Some(gate) = &mut self.gate {
            let (_, gate_errors) = gate.pump();
            errors.extend(gate_errors);
        }
        if let Some((service, inbox)) = &self.board {
            // A job this run doesn't track ended in an earlier one.
            let jobs = &self.jobs;
            let finished = |job| jobs.progress(job).is_none_or(|p| p.status.is_finished());
            errors.extend(service.pump(&**inbox, &finished));
        }
        if let Some(knowledge) = &self.knowledge {
            errors.extend(knowledge.pump());
        }
        EngineReport {
            errors,
            jobs_changed: report.changed,
            sessions_changed: report.changed || !report.new_sessions.is_empty() || watched,
        }
    }

    /// Takes the linter's and planner's replies, follows the missions'
    /// jobs and task graphs, and stops the jobs of missions out of budget.
    /// `facts` and `gates` are what the jobs and the gate said this frame.
    pub fn pump_missions(
        &mut self,
        facts: &[JobFact],
        gates: &HashMap<JobId, GateState>,
    ) -> MissionsPump {
        let Some(service) = &mut self.missions else {
            return MissionsPump::default();
        };
        let mut report = service.pump(facts, gates);
        let graphs = service.pump_graphs(&mut self.jobs, facts, gates);
        report.errors.extend(graphs.errors);
        report.cancel.extend(graphs.cancel);
        let mut errors = report.errors;
        for job in report.cancel {
            errors.extend(self.jobs.cancel(job).err());
        }
        MissionsPump {
            errors,
            graphs_changed: graphs.changed,
        }
    }

    /// Jobs running and queued now.
    pub fn busy(&self) -> (usize, usize) {
        let stats = self.jobs.stats();
        (stats.running, stats.queued)
    }

    /// Stops before swarmux does: nothing more starts, and the running jobs
    /// are stopped and marked to resume at the next start. Returns them.
    pub fn stop_for_restart(&mut self) -> AppResult<Vec<JobId>> {
        self.jobs.set_draining(true);
        self.jobs.interrupt_running()
    }
}
