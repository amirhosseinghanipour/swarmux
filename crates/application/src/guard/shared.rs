//! The policy, shared with the hook transport so it can answer at once.

use std::sync::{Arc, PoisonError, RwLock};
use std::time::Instant;

use swarmux_domain::{Decision, GuardPolicy, Pattern};

use crate::ports::HookRequest;

/// One policy for the UI thread and the transport's threads.
#[derive(Clone, Debug, Default)]
pub struct SharedPolicy(Arc<RwLock<GuardPolicy>>);

impl SharedPolicy {
    pub fn new(policy: GuardPolicy) -> SharedPolicy {
        SharedPolicy(Arc::new(RwLock::new(policy)))
    }

    /// The decision for `request`, and how long it took in microseconds.
    pub fn decide(&self, request: &HookRequest) -> (Decision, u64) {
        let start = Instant::now();
        let decision = self
            .0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .decide(&request.tool_name, &request.subject, &request.cwd);
        let micros = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
        (decision, micros)
    }

    pub fn add_allow(&self, rule: Pattern) {
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .add_allow(rule);
    }

    /// A copy of the current policy.
    pub fn snapshot(&self) -> GuardPolicy {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}
