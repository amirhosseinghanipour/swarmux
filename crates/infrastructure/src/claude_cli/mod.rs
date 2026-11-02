//! Adapter for Claude Code's command line (`claude -p` with stream-json).

pub mod command;
pub mod parse;
mod runner;

pub use runner::{ClaudeCli, GuardHook};
