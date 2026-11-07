//! swarmux domain layer: entities and pure business rules.
//!
//! Depends on nothing but `uuid`, `thiserror`, `sha2` and `regex`. No I/O, no clocks, no
//! frameworks; time is always passed in by the caller.

pub mod alarm;
pub mod anchor;
mod attention;
pub mod board;
pub mod budget;
pub mod contracts;
mod date;
pub mod dist;
mod error;
mod event;
pub mod evidence;
pub mod followup;
pub mod gate;
mod guard;
mod hook;
mod job;
mod json;
pub mod knowledge;
pub mod ledger;
pub mod mission;
mod model;
mod phase;
pub mod plan;
mod progress;
mod prompt;
mod review;
mod schedule;
mod session;
mod subagent;
pub mod taskgraph;
pub mod telemetry;
mod template;
mod tokens;
pub mod verifier;
pub mod workspace;
mod worktree;

pub use attention::{
    AttentionId, AttentionItem, AttentionKind, CallContext, HeldCall, Likelihood, Priority,
    STALL_AFTER_MS, is_destructive, rank,
};
pub use date::{iso_date, iso_minute, parse_iso_millis};
pub use error::DomainError;
pub use event::{Actor, AgentEvent, SessionState};
pub use guard::{
    AlarmKind, BUILTIN_ALLOW, BUILTIN_DENY, Decision, Detector, Gauges, GuardPolicy, Ladder,
    LadderPolicy, LoopBreaker, LoopTrip, NUDGE_MARK, NextRung, Pattern, Rung, RungOptions, Taken,
    Unmatched, Verdict, WatchAlarm, WatchLimits, error_signature, host_of, show_call,
};
pub use hook::HookEvent;
pub use job::{Job, JobId, JobResult, JobStatus};
pub use model::{Family, display_name};
pub use phase::{Phase, PhaseInput, phase_of};
pub use progress::{JobProgress, Lane, describe};
pub use prompt::{PromptDraft, PromptId, PromptTemplate, parse_tags};
pub use review::{
    REVIEW_SYSTEM, Review, ReviewState, ReviewVerdict, diff_files, parse_review, review_prompt,
};
pub use schedule::{Limits, plan, run_order};
pub use session::{DEFAULT_MODEL, DEFAULT_PERMISSION_MODE, Session, SessionId};
pub use subagent::{Subagent, SubagentStatus, ToolOutcome};
pub use template::{render, variables};
pub use tokens::{TokenTally, cache_read_ratio, estimate_tokens};
pub use workspace::{Repo, RepoRef, Stack, Workspace, WorkspaceId};
pub use worktree::{
    Checkpoint, CheckpointReason, Handoff, PortPlan, SetMember, WorktreeRecord, branch_name, slug,
};

/// Unix time in milliseconds.
pub type Millis = i64;
