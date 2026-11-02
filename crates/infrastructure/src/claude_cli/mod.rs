//! Adapter for Claude Code's command line (`claude -p` with stream-json).

pub mod command;
pub mod parse;
pub mod router;
mod runner;

pub use router::HaikuRouter;
pub use runner::{ClaudeCli, GuardHook};
