//! swarmux domain layer: entities and pure business rules.
//!
//! Depends on nothing but `uuid` and `thiserror`. No I/O, no clocks, no
//! frameworks; time is always passed in by the caller.

mod date;
mod error;
mod event;
mod job;
mod progress;
mod prompt;
mod schedule;
mod session;
mod template;

pub use date::{iso_date, iso_minute};
pub use error::DomainError;
pub use event::{Actor, AgentEvent};
pub use job::{Job, JobId, JobResult, JobStatus};
pub use progress::{JobProgress, Lane, describe};
pub use prompt::{PromptDraft, PromptId, PromptTemplate, parse_tags};
pub use schedule::{Limits, plan, run_order};
pub use session::{DEFAULT_MODEL, DEFAULT_PERMISSION_MODE, Session, SessionId};
pub use template::{render, variables};

/// Unix time in milliseconds.
pub type Millis = i64;
