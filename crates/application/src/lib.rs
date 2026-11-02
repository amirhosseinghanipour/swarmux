//! swarmux application layer: use cases, and the ports they need.
//!
//! Depends only on the domain. Adapters (SQLite, the `claude` CLI, the TUI)
//! live in other crates and plug in through the traits in [`ports`].

mod error;
mod jobs;
pub mod ports;
mod sessions;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use error::{AppError, AppResult};
pub use jobs::JobService;
pub use sessions::SessionService;
