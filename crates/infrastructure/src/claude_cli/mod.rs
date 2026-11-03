//! Adapter for Claude Code's command line (`claude -p` with stream-json).

pub mod command;
pub mod linter;
pub mod parse;
pub mod router;
mod runner;

pub use linter::HaikuLinter;
pub use router::HaikuRouter;
pub use runner::{ClaudeCli, GuardHook};
