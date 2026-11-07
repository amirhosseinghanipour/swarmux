//! Adapter for Claude Code's command line (`claude -p` with stream-json).

pub mod command;
pub mod contention;
pub mod diagnose;
pub mod extract;
pub mod linter;
pub mod parse;
pub mod planner;
pub mod router;
mod runner;

pub use diagnose::HaikuDiagnoser;
pub use extract::HaikuExtractor;
pub use linter::HaikuLinter;
pub use planner::ClaudePlanner;
pub use router::HaikuRouter;
pub use runner::{BoardMcp, ClaudeCli, GuardHook};
