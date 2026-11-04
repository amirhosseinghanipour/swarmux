//! The policy, shared with the hook transport so it can answer at once.

use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Instant;

use swarmux_domain::{Decision, GuardPolicy, Pattern, Verdict};

use crate::gate::WriteFence;
use crate::ports::HookRequest;
use crate::watchdog::NudgeGate;

/// One policy for the UI thread and the transport's threads.
#[derive(Clone, Default)]
pub struct SharedPolicy {
    policy: Arc<RwLock<GuardPolicy>>,
    /// Refuse writes (protected paths, a task's scope) before any rule
    /// is consulted, in the order they were added.
    fences: Vec<Arc<dyn WriteFence>>,
    /// Watchdog messages waiting for the agent's next tool call.
    nudges: Option<Arc<dyn NudgeGate>>,
}

impl fmt::Debug for SharedPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedPolicy")
            .field("policy", &self.policy)
            .field("fences", &self.fences.len())
            .field("nudges", &self.nudges.is_some())
            .finish()
    }
}

impl SharedPolicy {
    pub fn new(policy: GuardPolicy) -> SharedPolicy {
        SharedPolicy {
            policy: Arc::new(RwLock::new(policy)),
            fences: Vec::new(),
            nudges: None,
        }
    }

    /// A job with a watchdog message waiting gets it as the reason its next
    /// matching call is not run: the one way hooks can tell a running
    /// agent something.
    pub fn with_nudges(mut self, nudges: Arc<dyn NudgeGate>) -> SharedPolicy {
        self.nudges = Some(nudges);
        self
    }

    /// Writes `fence` refuses are denied whatever the rules say. Fences
    /// add up: the first that refuses gives the reason.
    pub fn with_fence(mut self, fence: Arc<dyn WriteFence>) -> SharedPolicy {
        self.fences.push(fence);
        self
    }

    /// The decision for `request`, and how long it took in microseconds.
    pub fn decide(&self, request: &HookRequest) -> (Decision, u64) {
        let start = Instant::now();
        let fenced = self.fences.iter().find_map(|f| {
            f.refuses(
                request.job_id,
                &request.tool_name,
                &request.subject,
                &request.cwd,
            )
        });
        let decision = match fenced {
            Some(rule) => Decision::new(Verdict::Deny, Some(rule)),
            None => self
                .policy
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .decide(&request.tool_name, &request.subject, &request.cwd),
        };
        // A call the rules deny anyway keeps its reason and the message waits.
        let nudge = match (&self.nudges, request.job_id, decision.verdict) {
            (_, _, Verdict::Deny) | (None, _, _) | (_, None, _) => None,
            (Some(n), Some(job), _) => n.take(job, &request.tool_name, &request.subject),
        };
        let decision = match nudge {
            Some(text) => Decision::new(Verdict::Deny, Some(text)),
            None => decision,
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
