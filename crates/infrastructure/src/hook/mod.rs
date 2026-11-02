//! Claude Code's PreToolUse hook, answered by the running swarmux over a
//! local socket.
//!
//! Each `claude` job gets a `--settings` hook that runs `swarmux hook pre-tool`;
//! that process forwards the call to [`HookServer`] and prints the decision.

mod client;
mod protocol;
mod server;
mod transport;

use std::path::Path;

pub use client::{pre_tool, pre_tool_main};
pub use protocol::{WireReply, WireRequest, subject_of};
pub use server::HookServer;
pub use transport::{ENV_JOB, ENV_PIPE, connect, endpoint_for};

/// The `--settings` JSON that makes `claude` run `exe hook pre-tool` before
/// every tool call, killed after `timeout_secs`.
pub fn hook_settings(exe: &Path, timeout_secs: u64) -> String {
    serde_json::json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "*",
                "hooks": [{
                    "type": "command",
                    "command": exe.to_string_lossy(),
                    "args": ["hook", "pre-tool"],
                    "timeout": timeout_secs,
                }]
            }]
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_use_the_exec_form() {
        let json = hook_settings(Path::new(r"C:\bin\swarmux.exe"), 130);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let hook = &v["hooks"]["PreToolUse"][0]["hooks"][0];
        assert_eq!(hook["type"], "command");
        assert_eq!(hook["command"], r"C:\bin\swarmux.exe");
        assert_eq!(hook["args"], serde_json::json!(["hook", "pre-tool"]));
        assert_eq!(hook["timeout"], 130);
        assert_eq!(v["hooks"]["PreToolUse"][0]["matcher"], "*");
    }
}
