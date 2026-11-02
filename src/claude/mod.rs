//! Driving Claude Code: stream-json events, job arguments, the job runner.

pub mod event;

pub use event::{Event, parse_line_all};
