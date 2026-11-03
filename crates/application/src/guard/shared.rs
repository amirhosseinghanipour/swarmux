//! The policy, shared with the hook transport so it can answer at once.

use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Instant;

use swarmux_domain::{Decision, GuardPolicy, Pattern, Verdict};

use crate::gate::WriteFence;
use crate::ports::HookRequest;

/// One policy for the UI thread and the transport's threads.
#[derive(Clone, Default)]
pub struct SharedPolicy {
    policy: Arc<RwLock<GuardPolicy>>,
    /// Refuses writes to protected paths before any rule is consulted.
    fence: Option<Arc<dyn WriteFence>>,
}

impl fmt::Debug for SharedPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedPolicy")
            .field("policy", &self.policy)
            .field("fence", &self.fence.is_some())
            .finish()
    }
}

impl SharedPolicy {
    pub fn new(policy: GuardPolicy) -> SharedPolicy {
        SharedPolicy {
            policy: Arc::new(RwLock::new(policy)),
            fence: None,
        }
    }

    /// Writes `fence` refuses are denied whatever the rules say.
    pub fn with_fence(mut self, fence: Arc<dyn WriteFence>) -> SharedPolicy {
        self.fence = Some(fence);
        self
    }

    /// The decision for `request`, and how long it took in microseconds.
    pub fn decide(&self, request: &HookRequest) -> (Decision, u64) {
        let start = Instant::now();
        let fenced = self
            .fence
            .as_ref()
            .and_then(|f| f.refuses(&request.tool_name, &request.subject, &request.cwd));
        let decision = match fenced {
            Some(rule) => Decision::new(Verdict::Deny, Some(rule)),
            None => self
                .policy
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .decide(&request.tool_name, &request.subject, &request.cwd),
        };
        let micros = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
        (decision, micros)
    }

    pub fn add_allow(&self, rule: Pattern) {
        self.policy
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .add_allow(rule);
    }

    /// A copy of the current policy.
    pub fn snapshot(&self) -> GuardPolicy {
        self.policy
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}
