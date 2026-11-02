//! Driving Claude Code: stream-json events, job arguments, the job runner.

pub mod command;
pub mod event;
pub mod runner;

pub use command::{JobSpec, build_args, user_message};
pub use event::{Event, parse_line_all};
pub use runner::{RunnerMsg, run_job};
