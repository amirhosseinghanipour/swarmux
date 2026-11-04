use std::cell::RefCell;
use std::collections::HashMap;

use swarmux_application::ports::{
    AgentRunner, HookInbox, HookRequest, InboxItem, RunRequest, RunSink, RunnerMsg,
};
use swarmux_domain::{Decision, JobId};

/// A `claude` that does what the script says. Starting a job only keeps its
/// sink; the demo's director sends the job's events through [`emit`].
///
/// [`emit`]: ScriptedRunner::emit
#[derive(Default)]
pub struct ScriptedRunner {
    started: RefCell<HashMap<JobId, (RunRequest, RunSink)>>,
}

impl ScriptedRunner {
    /// Whether the job has been started and not stopped since.
    pub fn is_started(&self, job: JobId) -> bool {
        self.started.borrow().contains_key(&job)
    }

    /// What a started job was asked to run.
    pub fn request_of(&self, job: JobId) -> Option<RunRequest> {
        self.started.borrow().get(&job).map(|(r, _)| r.clone())
    }

    /// Sends `msg` as the job's own output. False when it has not started.
    pub fn emit(&self, job: JobId, msg: RunnerMsg) -> bool {
        match self.started.borrow().get(&job) {
            Some((_, sink)) => {
                sink(msg);
                true
            }
            None => false,
        }
    }
}

impl AgentRunner for ScriptedRunner {
    fn start(&self, request: RunRequest, sink: RunSink) {
        self.started
            .borrow_mut()
            .insert(request.job_id, (request, sink));
    }

    fn cancel(&self, job: JobId) {
        self.started.borrow_mut().remove(&job);
    }

    fn command_line(&self, _request: &RunRequest) -> Option<String> {
        Some("claude (scripted demo, nothing is run)".into())
    }
}

/// Where the demo's tool calls arrive, in place of Claude Code's hook.
#[derive(Default)]
pub struct ScriptedInbox {
    queue: RefCell<Vec<InboxItem>>,
    answers: RefCell<Vec<(u64, Decision)>>,
}

impl ScriptedInbox {
    /// A tool call the policy has already answered (`Some`) or one the
    /// user has to answer (`None`).
    pub fn push(&self, request: HookRequest, answered: Option<(Decision, u64)>) {
        self.queue
            .borrow_mut()
            .push(InboxItem { request, answered });
    }

    /// The answers given to held calls, oldest first.
    pub fn answers(&self) -> Vec<(u64, Decision)> {
        self.answers.borrow().clone()
    }
}

impl HookInbox for ScriptedInbox {
    fn poll(&self) -> Vec<InboxItem> {
        std::mem::take(&mut *self.queue.borrow_mut())
    }

    fn respond(&self, request_id: u64, decision: &Decision) {
        self.answers
            .borrow_mut()
            .push((request_id, decision.clone()));
    }
}
