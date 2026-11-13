//! Claude Code's lifecycle hooks, carried to the running swarmux over a
//! local socket.
//!
//! Each `claude` job gets `--settings` hooks that run `swarmux hook <event>`;
//! that process forwards the event to [`HookServer`] and prints the answer.
//! PreToolUse is decided by the Guard; the other events are recorded.

mod client;
mod fallback;
mod protocol;
mod server;
mod statusline;
mod transport;

use std::path::Path;

use swarmux_domain::HookEvent;

pub use client::{HookEnv, board_call, hook_main, pre_tool, pre_tool_in, pre_tool_main, run_hook};
pub use fallback::{ENV_FAIL, ENV_SPOOL, FileSpool, append as append_fallback, spool_file};
pub use protocol::{BOARD_EVENT, WireReply, WireRequest, subject_of};
pub use server::{HookServer, Responders};
pub use statusline::{SpoolFeed, run_statusline, spool_dir, statusline_main};

/// The `statusLine` setting that runs `exe statusline`, as JSON to merge
/// into a Claude Code settings file.
pub fn statusline_settings(exe: &Path) -> String {
    serde_json::json!({ "statusLine": { "type": "command", "command": statusline_command(exe) } })
        .to_string()
}
pub(crate) use transport::user as user_name;
pub use transport::{ENV_JOB, ENV_PIPE, connect, endpoint_for};

/// Claude Code kills a lifecycle hook after this long. They answer at once.
const LIFECYCLE_TIMEOUT_SECS: u64 = 30;
/// Claude Code kills a `Stop` or `SubagentStop` hook after this long: the
/// done-gate runs the repository's checks before it answers. A gate still
/// running then lets the agent stop.
pub const STOP_TIMEOUT_SECS: u64 = 1800;

/// The `--settings` JSON that makes `claude` run `exe hook <event>` for
/// every lifecycle event swarmux listens to, and `exe statusline` as the
/// status line. PreToolUse is killed after `timeout_secs` (it may wait for
/// the user); the events nobody waits for are registered asynchronous.
pub fn hook_settings(exe: &Path, timeout_secs: u64) -> String {
    hook_settings_with(exe, timeout_secs, false)
}

/// Like [`hook_settings`]; with `on_failure_block` the PreToolUse hook also
/// carries `"onFailure": "block"`, which Claude Code versions that know it
/// answer a failing hook with (a refusal rather than letting the call run).
/// Versions that don't know the key ignore it. Which versions know it is
/// not detected here: the key is added only when asked for.
pub fn hook_settings_with(exe: &Path, timeout_secs: u64, on_failure_block: bool) -> String {
    let mut hooks = serde_json::Map::new();
    for event in HookEvent::ALL {
        let mut command = serde_json::json!({
            "type": "command",
            "command": exe.to_string_lossy(),
            "args": ["hook", arg_of(event)],
            "timeout": timeout_of(event, timeout_secs),
        });
        if !event.is_blocking() {
            command["async"] = true.into();
        }
        if on_failure_block && event == HookEvent::PreToolUse {
            command["onFailure"] = "block".into();
        }
        let mut group = serde_json::json!({ "hooks": [command] });
        if has_matcher(event) {
            group["matcher"] = "*".into();
        }
        hooks.insert(event.as_str().to_string(), serde_json::json!([group]));
    }
    serde_json::json!({
        "hooks": hooks,
        "statusLine": { "type": "command", "command": statusline_command(exe) },
    })
    .to_string()
}

/// The status line command: the executable quoted, with forward slashes so
/// a shell reads the path as written.
fn statusline_command(exe: &Path) -> String {
    format!(
        "\"{}\" statusline",
        exe.to_string_lossy().replace('\\', "/")
    )
}

/// How long Claude Code lets the hook for `event` run.
fn timeout_of(event: HookEvent, pre_tool_secs: u64) -> u64 {
    match event {
        HookEvent::PreToolUse => pre_tool_secs,
        HookEvent::Stop | HookEvent::SubagentStop => STOP_TIMEOUT_SECS,
        _ => LIFECYCLE_TIMEOUT_SECS,
    }
}

/// The command line name of `event` in the injected settings.
fn arg_of(event: HookEvent) -> String {
    match event {
        HookEvent::PreToolUse => "pre-tool".to_string(),
        other => other.kebab(),
    }
}

/// Events whose hooks can be filtered (by tool, source, trigger, …).
/// `Stop` has no filter, and a matcher on it would be ignored or refused.
fn has_matcher(event: HookEvent) -> bool {
    event != HookEvent::Stop
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn settings() -> Value {
        serde_json::from_str(&hook_settings(Path::new(r"C:\bin\swarmux.exe"), 130)).unwrap()
    }

    #[test]
    fn pre_tool_use_keeps_its_exec_form() {
        let v = settings();
        let hook = &v["hooks"]["PreToolUse"][0]["hooks"][0];
        assert_eq!(hook["type"], "command");
        assert_eq!(hook["command"], r"C:\bin\swarmux.exe");
        assert_eq!(hook["args"], serde_json::json!(["hook", "pre-tool"]));
        assert_eq!(hook["timeout"], 130);
        assert_eq!(hook["async"], Value::Null);
        assert_eq!(v["hooks"]["PreToolUse"][0]["matcher"], "*");
    }

    #[test]
    fn on_failure_block_is_added_to_the_pre_tool_hook_only_when_asked() {
        let with: Value =
            serde_json::from_str(&hook_settings_with(Path::new("/x/swarmux"), 130, true)).unwrap();
        assert_eq!(
            with["hooks"]["PreToolUse"][0]["hooks"][0]["onFailure"],
            "block"
        );
        assert_eq!(
            with["hooks"]["Stop"][0]["hooks"][0]["onFailure"],
            Value::Null
        );
        assert_eq!(
            settings()["hooks"]["PreToolUse"][0]["hooks"][0]["onFailure"],
            Value::Null
        );
    }

    #[test]
    fn every_event_is_registered_and_only_waited_for_ones_are_sync() {
        let v = settings();
        let hooks = v["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), HookEvent::ALL.len());
        for event in HookEvent::ALL {
            let hook = &hooks[event.as_str()][0]["hooks"][0];
            assert_eq!(hook["command"], r"C:\bin\swarmux.exe", "{event:?}");
            let arg = hook["args"][1].as_str().unwrap();
            assert_eq!(HookEvent::parse(arg), Some(event), "{event:?}");
            if event.is_blocking() {
                assert_eq!(hook["async"], Value::Null, "{event:?}");
            } else {
                assert_eq!(hook["async"], true, "{event:?}");
            }
        }
    }

    #[test]
    fn the_status_line_runs_the_sidecar() {
        let v = settings();
        assert_eq!(v["statusLine"]["type"], "command");
        assert_eq!(
            v["statusLine"]["command"],
            r#""C:/bin/swarmux.exe" statusline"#
        );
    }

    #[test]
    fn matchers_where_they_apply() {
        let v = settings();
        for event in HookEvent::ALL {
            let group = &v["hooks"][event.as_str()][0];
            if event == HookEvent::Stop {
                assert_eq!(group.get("matcher"), None);
            } else {
                assert_eq!(group["matcher"], "*", "{event:?}");
            }
        }
    }

    #[test]
    fn settings_snapshot_for_two_events() {
        let v = settings();
        assert_eq!(
            v["hooks"]["PostToolUseFailure"][0],
            serde_json::json!({
                "matcher": "*",
                "hooks": [{
                    "type": "command",
                    "command": r"C:\bin\swarmux.exe",
                    "args": ["hook", "post-tool-use-failure"],
                    "timeout": 30,
                    "async": true,
                }]
            })
        );
        assert_eq!(
            v["hooks"]["Stop"][0],
            serde_json::json!({
                "hooks": [{
                    "type": "command",
                    "command": r"C:\bin\swarmux.exe",
                    "args": ["hook", "stop"],
                    "timeout": 1800,
                }]
            })
        );
    }
}
