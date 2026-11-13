//! The console read model: the data the Work and Deck screens read, and
//! that a daemon API or a phone client can read after them.
//!
//! No terminal types live here. [`ConsoleService::snapshot`] turns what
//! swarmux already keeps (missions and their plans and task graphs, jobs,
//! the gate's verdicts, the attention inbox, the Guard's holds and
//! decisions, the board's claims, the repositories' contracts, sealed
//! evidence, the limit windows) into a [`ConsoleSnapshot`]: ranked groups
//! of [`OutcomeView`]s, the Guard's holds with the agent's own words, the
//! Deck's graph and the figures of the header. Nothing is invented and
//! nothing new is stored: it is a projection, rebuilt when its inputs
//! change.
//!
//! The rules (ranking, allowed actions, edges) are the domain's
//! (`swarmux_domain::console`); this module gathers the facts, applies
//! them and fixes the wire shape ([`view`]).
//!
//! # Cost
//!
//! A snapshot is not rebuilt per frame. [`ConsoleService::snapshot`]
//! first computes a stamp over the inputs (one pass over the jobs, the
//! missions, the open asks and the gate's verdicts: a few hundred items)
//! and returns the snapshot it has when the stamp is the same. A rebuild
//! reads:
//!
//! - the facts already in memory (`JobFact`s, missions, plans, verdicts);
//! - one sealed bundle per delivered mission, once (cached by mission);
//! - the tail of one job's stored stream per held or denied call: a
//!   bounded, indexed read of at most [`STREAM_WINDOW`] events, however
//!   long the job ran;
//! - the repositories' contract index, once per rescan (cached by the
//!   index it was derived from).
//!
//! The ledger is never scanned: the ledger range of an outcome comes from
//! its sealed bundle, so a ledger of tens of thousands of entries costs
//! the same as an empty one. The landed group holds its newest
//! [`DEFAULT_LANDED`] outcomes in full; the rest is only counted.

mod deck;
mod hold;
mod outcome;
mod service;
#[cfg(test)]
mod tests;
mod view;

pub use hold::STREAM_WINDOW;
pub use service::{ConsoleService, DEFAULT_LANDED, Extra, Gathered, Sources};
pub use view::*;
