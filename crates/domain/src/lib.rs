//! swarmux domain layer: entities and pure business rules.
//!
//! Depends on nothing but `uuid` and `thiserror`. No I/O, no clocks, no
//! frameworks; time is always passed in by the caller.

mod error;
mod event;
mod job;
mod progress;
mod session;

pub use error::DomainError;
pub use event::{Actor, AgentEvent};
pub use job::{JobId, JobResult, JobStatus};
pub use progress::{JobProgress, Lane, describe};
pub use session::{DEFAULT_MODEL, DEFAULT_PERMISSION_MODE, Session, SessionId};

/// Unix time in milliseconds.
pub type Millis = i64;
