//! The done-gate's adapters: `.swarmux/gates.toml` (or checks guessed from
//! the repository), protected-file hashes and the process checker.

mod checker;
mod config;

pub use checker::{OUTPUT_MAX, ProcessChecker};
pub use config::{FsGateSource, GATES_FILE, parse_gates};
