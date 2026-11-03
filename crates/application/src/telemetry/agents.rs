//! Sessions swarmux didn't start, found through `claude agents --json`.

use swarmux_domain::telemetry::ExternalSession;

/// Lists the sessions Claude Code itself knows. Read-only; called from a
/// background thread, so it may take its time.
pub trait AgentsPoll: Send + Sync {
    /// The sessions now, or why they couldn't be listed.
    fn list(&self) -> Result<Vec<ExternalSession>, String>;
}
