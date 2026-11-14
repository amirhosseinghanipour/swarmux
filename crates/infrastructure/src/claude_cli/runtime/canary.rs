//! Canaries: tiny real `claude` calls that try the policy against the
//! Claude Code that is installed.
//!
//! Each launches `claude -p` in a scratch folder under the system temp
//! folder, with the settings sources and MCP flags of the profile under test,
//! and with `swarmux hook canary <mode>` as the PreToolUse hook. The model is
//! asked to run `touch <file>`, which an allow rule in the generated
//! settings would let through, so the only thing that can stop it is what is
//! being tried. The canary then looks at three things: whether the file
//! exists, whether the hook ran (it logs every call it gets), and what came
//! back to the model. A canary that cannot tell says `inconclusive`; the
//! callers treat that as a failure wherever a run depends on it.
//!
//! What the real calls showed on 2.1.323 is in `guide/claude-code.md`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use swarmux_domain::runtime::{CanaryCheck, CheckStatus};

use crate::hook::CANARY_REASON;

/// The model a canary asks: the smallest, since it only has to make a call.
pub const CANARY_MODEL: &str = "haiku";
/// How long the hook of a canary may run before Claude Code ends it.
const HOOK_TIMEOUT_SECS: u64 = 5;

/// The calls the canaries make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    /// The hook denies; the call must not run.
    Deny,
    /// The hook exits 1; with `onFailure: "block"` the call must not run.
    HookExit1,
    /// The hook never answers; with `onFailure: "block"` the timeout must
    /// stop the call.
    HookTimeout,
    /// The hook prints text that is not JSON.
    HookInvalidJson,
    /// A subagent makes the call; the hook must see it.
    Subagent,
}

impl Scenario {
    pub const ALL: [Scenario; 5] = [
        Scenario::Deny,
        Scenario::HookExit1,
        Scenario::HookTimeout,
        Scenario::HookInvalidJson,
        Scenario::Subagent,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Scenario::Deny => "deny",
            Scenario::HookExit1 => "hook_exit_1",
            Scenario::HookTimeout => "hook_timeout",
            Scenario::HookInvalidJson => "hook_invalid_json",
            Scenario::Subagent => "subagent",
        }
    }

    /// A start may not depend on a canary that is only informative.
    pub fn required(self) -> bool {
        matches!(
            self,
            Scenario::Deny | Scenario::HookExit1 | Scenario::HookTimeout
        )
    }

    fn hook_mode(self) -> &'static str {
        match self {
            Scenario::Deny | Scenario::Subagent => "deny",
            Scenario::HookExit1 => "exit1",
            Scenario::HookTimeout => "hang",
            Scenario::HookInvalidJson => "badjson",
        }
    }

    fn file(self) -> String {
        format!("canary-{}.txt", self.name())
    }

    fn prompt(self) -> String {
        let file = self.file();
        match self {
            Scenario::Subagent => format!(
                "Use the Task tool once, with a general-purpose subagent, and tell it to run the Bash command: touch {file} . Then reply with the single word done."
            ),
            _ => format!(
                "Run the Bash command: touch {file} . Then reply with the single word done."
            ),
        }
    }

    fn tools(self) -> &'static str {
        match self {
            Scenario::Subagent => "Bash,Task",
            _ => "Bash",
        }
    }
}

/// How the canaries launch `claude`.
#[derive(Clone, Debug)]
pub struct CanarySetup {
    pub bin: PathBuf,
    /// The swarmux executable that runs `hook canary`.
    pub exe: PathBuf,
    /// `--setting-sources` of the profile under test (`None`: the default).
    pub sources: Option<String>,
    /// `--strict-mcp-config`, as the profile passes it.
    pub strict_mcp: bool,
    /// The hook carries `onFailure: "block"`.
    pub on_failure_block: bool,
    pub timeout: Duration,
    /// Extra environment for the call (tests tell the fake `claude` how to
    /// behave this way).
    pub env: Vec<(String, String)>,
}

/// What one canary run left behind.
#[derive(Debug, Default)]
pub struct Observed {
    pub timed_out: bool,
    pub spawn_error: Option<String>,
    pub exit_code: Option<i32>,
    /// `(tool, is_error, text)` of what the model was told for each call.
    pub tool_results: Vec<(bool, String)>,
    /// Tools the model called.
    pub tool_calls: Vec<String>,
    /// `(tool, agent_id)` of each call the hook got.
    pub hook_calls: Vec<(String, String)>,
    /// The file the call would have made exists.
    pub file_made: bool,
    pub stderr_tail: String,
}

/// The canary's scratch folder, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> std::io::Result<Scratch> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir =
            std::env::temp_dir().join(format!("swarmux-canary-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        Ok(Scratch(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The settings a canary passes: the canary hook, an allow rule that would
/// let the call through, and hooks pinned on.
pub fn canary_settings(setup: &CanarySetup, scenario: Scenario, log: &Path) -> String {
    let mut hook = json!({
        "type": "command",
        "command": setup.exe.to_string_lossy(),
        "args": ["hook", "canary", scenario.hook_mode(), "--log", log.to_string_lossy()],
        "timeout": HOOK_TIMEOUT_SECS,
    });
    if setup.on_failure_block {
        hook["onFailure"] = "block".into();
    }
    // The subagent canary lets the call that starts the subagent through
    // (the hook is for the Bash call it makes); the others guard every tool,
    // as the Guard's hook does.
    let (matcher, allow) = match scenario {
        Scenario::Subagent => ("Bash", json!(["Bash(touch *)", "Agent"])),
        _ => ("*", json!(["Bash(touch *)"])),
    };
    json!({
        "hooks": { "PreToolUse": [{ "matcher": matcher, "hooks": [hook] }] },
        "permissions": { "allow": allow },
        "disableAllHooks": false,
    })
    .to_string()
}

/// The arguments of the canary's `claude` call.
pub fn canary_args(setup: &CanarySetup, scenario: Scenario, settings: &Path) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        &scenario.prompt(),
        "--model",
        CANARY_MODEL,
        "--max-turns",
        "4",
        "--tools",
        scenario.tools(),
        "--permission-mode",
        "default",
        "--output-format",
        "stream-json",
        "--verbose",
        "--no-session-persistence",
    ]
    .map(String::from)
    .to_vec();
    if let Some(sources) = &setup.sources {
        args.extend(["--setting-sources".into(), sources.clone()]);
    }
    if setup.strict_mcp {
        args.push("--strict-mcp-config".into());
    }
    args.extend(["--settings".into(), settings.to_string_lossy().into_owned()]);
    args
}

/// Runs one canary and judges it.
pub fn run(setup: &CanarySetup, scenario: Scenario) -> CanaryCheck {
    let observed = observe(setup, scenario);
    judge(scenario, setup.on_failure_block, &observed)
}

fn observe(setup: &CanarySetup, scenario: Scenario) -> Observed {
    let mut seen = Observed::default();
    let scratch = match Scratch::new() {
        Ok(s) => s,
        Err(e) => {
            seen.spawn_error = Some(format!("no scratch folder: {e}"));
            return seen;
        }
    };
    let log = scratch.0.join("hook-calls.jsonl");
    let settings_path = scratch.0.join("canary-settings.json");
    if let Err(e) = std::fs::write(&settings_path, canary_settings(setup, scenario, &log)) {
        seen.spawn_error = Some(format!("cannot write the canary settings: {e}"));
        return seen;
    }
    let mut cmd = Command::new(&setup.bin);
    cmd.args(canary_args(setup, scenario, &settings_path))
        .current_dir(&scratch.0)
        .envs(setup.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::process::hide_std(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            seen.spawn_error = Some(format!("{}: {e}", setup.bin.display()));
            return seen;
        }
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_reader = std::thread::spawn(move || read_all(stdout));
    let err_reader = std::thread::spawn(move || read_all(stderr));
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                seen.exit_code = status.code();
                break;
            }
            Ok(None) if started.elapsed() >= setup.timeout => {
                seen.timed_out = true;
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => {
                seen.spawn_error = Some(e.to_string());
                break;
            }
        }
    }
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    seen.stderr_tail = tail(&stderr, 400);
    read_stream(&stdout, &mut seen);
    seen.hook_calls = read_hook_log(&log);
    seen.file_made = scratch.0.join(scenario.file()).exists();
    seen
}

fn read_all(pipe: Option<impl std::io::Read>) -> String {
    let mut text = String::new();
    if let Some(mut p) = pipe {
        let mut bytes = Vec::new();
        let _ = p.read_to_end(&mut bytes);
        text = String::from_utf8_lossy(&bytes).into_owned();
    }
    text
}

fn tail(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}

/// Reads the stream-json lines: the calls the model made and what it was
/// told came back.
pub fn read_stream(stdout: &str, seen: &mut Observed) {
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let content = v.pointer("/message/content").and_then(Value::as_array);
        for part in content.into_iter().flatten() {
            match part.get("type").and_then(Value::as_str) {
                Some("tool_use") => {
                    if let Some(name) = part.get("name").and_then(Value::as_str) {
                        seen.tool_calls.push(name.to_string());
                    }
                }
                Some("tool_result") => {
                    let is_error = part.get("is_error").and_then(Value::as_bool) == Some(true);
                    let text = match part.get("content") {
                        Some(Value::String(s)) => s.clone(),
                        Some(other) => other.to_string(),
                        None => String::new(),
                    };
                    seen.tool_results.push((is_error, text));
                }
                _ => {}
            }
        }
    }
}

fn read_hook_log(path: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(|v| {
            let text = |k: &str| {
                v.get(k)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            (text("tool"), text("agent_id"))
        })
        .collect()
}

/// What `seen` means for `scenario`.
pub fn judge(scenario: Scenario, on_failure_block: bool, seen: &Observed) -> CanaryCheck {
    let (status, detail) = verdict(scenario, on_failure_block, seen);
    CanaryCheck {
        name: scenario.name().to_string(),
        status,
        detail,
        required: scenario.required(),
    }
}

fn verdict(scenario: Scenario, on_failure_block: bool, seen: &Observed) -> (CheckStatus, String) {
    use CheckStatus::{Fail, Inconclusive, Note, Pass};
    if let Some(e) = &seen.spawn_error {
        return (Inconclusive, format!("could not run claude: {e}"));
    }
    // A call that ran is the worst answer whatever the scenario expected.
    let ran = seen.file_made;
    let hook_ran = !seen.hook_calls.is_empty();
    let said_why = seen
        .tool_results
        .iter()
        .any(|(_, t)| t.contains(CANARY_REASON));
    if seen.timed_out && !ran && !hook_ran {
        return (
            Inconclusive,
            "timed out before the call was made".to_string(),
        );
    }
    match scenario {
        Scenario::Deny => {
            if ran {
                (Fail, "the call ran although the hook denied it".to_string())
            } else if hook_ran {
                let why = if said_why {
                    "the hook's reason came back to the model"
                } else {
                    "the call did not run"
                };
                (
                    Pass,
                    format!("the hook ran and the call was stopped: {why}"),
                )
            } else if !seen.tool_calls.is_empty() {
                (
                    Fail,
                    "the call was made and the hook never ran (its settings were dropped)"
                        .to_string(),
                )
            } else {
                (Inconclusive, no_call(seen))
            }
        }
        Scenario::HookExit1 | Scenario::HookTimeout => {
            let how = if scenario == Scenario::HookExit1 {
                "exited 1"
            } else {
                "did not answer before its timeout"
            };
            if !on_failure_block {
                return (
                    Note,
                    format!("onFailure is not set, so a hook that {how} is not expected to block"),
                );
            }
            if ran {
                (
                    Fail,
                    format!("the hook {how} and the call ran anyway, with onFailure=block"),
                )
            } else if hook_ran {
                (Pass, format!("the hook {how} and the call was blocked"))
            } else if !seen.tool_calls.is_empty() {
                (Fail, "the call was made and the hook never ran".to_string())
            } else {
                (Inconclusive, no_call(seen))
            }
        }
        Scenario::HookInvalidJson => {
            if !hook_ran {
                (Inconclusive, no_call(seen))
            } else if ran {
                (
                    Note,
                    "Claude Code ran the call although the hook printed invalid JSON, even with onFailure=block. swarmux's hook always prints valid JSON or exits with a code, so this is a limit of Claude Code, not a failure"
                        .to_string(),
                )
            } else {
                (
                    Pass,
                    "the hook printed invalid JSON and the call was blocked".to_string(),
                )
            }
        }
        Scenario::Subagent => {
            let by_subagent = seen
                .hook_calls
                .iter()
                .any(|(tool, agent)| tool == "Bash" && !agent.is_empty());
            if ran {
                (
                    Fail,
                    "a subagent's call ran although the hook denied it".to_string(),
                )
            } else if by_subagent {
                (
                    Pass,
                    "the hook was asked about a subagent's call, with its agent_id, and denied it"
                        .to_string(),
                )
            } else if hook_ran {
                (
                    Inconclusive,
                    "the hook saw only the lead's calls; no subagent made one".to_string(),
                )
            } else {
                (
                    Inconclusive,
                    "the model did not start a subagent".to_string(),
                )
            }
        }
    }
}

fn no_call(seen: &Observed) -> String {
    if seen.timed_out {
        "timed out before a call was made".to_string()
    } else {
        let tail = if seen.stderr_tail.is_empty() {
            String::new()
        } else {
            format!(" ({})", seen.stderr_tail.replace('\n', " "))
        };
        format!("the model made no tool call{tail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(on_failure_block: bool) -> CanarySetup {
        CanarySetup {
            bin: "claude".into(),
            exe: "/bin/swarmux".into(),
            sources: Some("user".into()),
            strict_mcp: true,
            on_failure_block,
            timeout: Duration::from_secs(60),
            env: Vec::new(),
        }
    }

    fn seen() -> Observed {
        Observed::default()
    }

    #[test]
    fn the_settings_carry_the_canary_hook_and_an_allow_rule() {
        let v: Value = serde_json::from_str(&canary_settings(
            &setup(true),
            Scenario::HookExit1,
            Path::new("/t/log"),
        ))
        .unwrap();
        let hook = &v["hooks"]["PreToolUse"][0]["hooks"][0];
        assert_eq!(hook["command"], "/bin/swarmux");
        assert_eq!(
            hook["args"],
            json!(["hook", "canary", "exit1", "--log", "/t/log"])
        );
        assert_eq!(hook["onFailure"], "block");
        assert_eq!(hook["timeout"], HOOK_TIMEOUT_SECS);
        assert_eq!(v["permissions"]["allow"][0], "Bash(touch *)");
        assert_eq!(v["hooks"]["PreToolUse"][0]["matcher"], "*");
        assert_eq!(v["disableAllHooks"], false);
        let sub: Value = serde_json::from_str(&canary_settings(
            &setup(true),
            Scenario::Subagent,
            Path::new("/t/log"),
        ))
        .unwrap();
        assert_eq!(sub["hooks"]["PreToolUse"][0]["matcher"], "Bash");
        assert_eq!(sub["permissions"]["allow"][1], "Agent");
        let without: Value = serde_json::from_str(&canary_settings(
            &setup(false),
            Scenario::Deny,
            Path::new("/t/log"),
        ))
        .unwrap();
        assert_eq!(
            without["hooks"]["PreToolUse"][0]["hooks"][0]["onFailure"],
            Value::Null
        );
    }

    #[test]
    fn the_call_is_small_and_takes_the_profile_s_flags() {
        let args = canary_args(&setup(true), Scenario::Deny, Path::new("/t/s.json"));
        let pair = |a: &str, b: &str| args.windows(2).any(|w| w[0] == a && w[1] == b);
        assert!(pair("--model", "haiku"));
        assert!(pair("--max-turns", "4"));
        assert!(pair("--tools", "Bash"));
        assert!(pair("--setting-sources", "user"));
        assert!(pair("--settings", "/t/s.json"));
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        assert!(args.contains(&"--no-session-persistence".to_string()));
        let sub = canary_args(&setup(true), Scenario::Subagent, Path::new("/t/s.json"));
        assert!(
            sub.windows(2)
                .any(|w| w[0] == "--tools" && w[1] == "Bash,Task")
        );
        let mut open = setup(true);
        open.sources = None;
        open.strict_mcp = false;
        let args = canary_args(&open, Scenario::Deny, Path::new("/t/s.json"));
        assert!(!args.contains(&"--setting-sources".to_string()));
        assert!(!args.contains(&"--strict-mcp-config".to_string()));
    }

    #[test]
    fn a_denied_call_with_the_hook_seen_passes() {
        let mut s = seen();
        s.hook_calls.push(("Bash".into(), String::new()));
        s.tool_results
            .push((true, format!("PreToolUse:Bash hook error: {CANARY_REASON}")));
        let c = judge(Scenario::Deny, true, &s);
        assert_eq!(c.status, CheckStatus::Pass, "{}", c.detail);
        assert!(c.required);
        assert!(c.detail.contains("reason came back"));
    }

    #[test]
    fn a_call_that_ran_fails_every_scenario_that_expected_it_stopped() {
        for scenario in [
            Scenario::Deny,
            Scenario::HookExit1,
            Scenario::HookTimeout,
            Scenario::Subagent,
        ] {
            let mut s = seen();
            s.hook_calls.push(("Bash".into(), "a1".into()));
            s.file_made = true;
            let c = judge(scenario, true, &s);
            assert_eq!(c.status, CheckStatus::Fail, "{scenario:?}: {}", c.detail);
        }
    }

    #[test]
    fn a_hook_that_never_ran_is_a_failure_when_the_call_was_made() {
        let mut s = seen();
        s.tool_calls.push("Bash".into());
        let c = judge(Scenario::Deny, true, &s);
        assert_eq!(c.status, CheckStatus::Fail);
        assert!(c.detail.contains("never ran"));
    }

    #[test]
    fn no_call_at_all_is_inconclusive_not_a_pass() {
        let c = judge(Scenario::Deny, true, &seen());
        assert_eq!(c.status, CheckStatus::Inconclusive);
        let mut timed = seen();
        timed.timed_out = true;
        assert_eq!(
            judge(Scenario::Deny, true, &timed).status,
            CheckStatus::Inconclusive
        );
        let mut broken = seen();
        broken.spawn_error = Some("no such file".into());
        let c = judge(Scenario::Deny, true, &broken);
        assert_eq!(c.status, CheckStatus::Inconclusive);
        assert!(c.detail.contains("no such file"));
    }

    #[test]
    fn a_failing_hook_must_block_when_on_failure_is_set() {
        let mut blocked = seen();
        blocked.hook_calls.push(("Bash".into(), String::new()));
        assert_eq!(
            judge(Scenario::HookExit1, true, &blocked).status,
            CheckStatus::Pass
        );
        assert_eq!(
            judge(Scenario::HookTimeout, true, &blocked).status,
            CheckStatus::Pass
        );
        let mut ran = blocked;
        ran.file_made = true;
        assert_eq!(
            judge(Scenario::HookExit1, true, &ran).status,
            CheckStatus::Fail
        );
        // Without onFailure nothing is expected, and it is said so.
        assert_eq!(
            judge(Scenario::HookExit1, false, &ran).status,
            CheckStatus::Note
        );
    }

    #[test]
    fn invalid_json_that_lets_the_call_run_is_a_note_not_a_failure() {
        let mut s = seen();
        s.hook_calls.push(("Bash".into(), String::new()));
        s.file_made = true;
        let c = judge(Scenario::HookInvalidJson, true, &s);
        assert_eq!(c.status, CheckStatus::Note);
        assert!(!c.required);
        s.file_made = false;
        assert_eq!(
            judge(Scenario::HookInvalidJson, true, &s).status,
            CheckStatus::Pass
        );
    }

    #[test]
    fn a_subagent_call_needs_its_agent_id_in_the_hook_log() {
        let mut lead_only = seen();
        lead_only.hook_calls.push(("Bash".into(), String::new()));
        assert_eq!(
            judge(Scenario::Subagent, true, &lead_only).status,
            CheckStatus::Inconclusive
        );
        let mut by_sub = seen();
        by_sub.hook_calls.push(("Bash".into(), "agent-1".into()));
        let c = judge(Scenario::Subagent, true, &by_sub);
        assert_eq!(c.status, CheckStatus::Pass);
        assert!(!c.required);
    }

    #[test]
    fn the_stream_gives_calls_and_results() {
        let stream = [
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"touch x"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":"PreToolUse:Bash hook error: swarmux canary: denied on purpose"}]}}"#,
            "not json",
            r#"{"type":"result","subtype":"success"}"#,
        ]
        .join("\n");
        let mut s = seen();
        read_stream(&stream, &mut s);
        assert_eq!(s.tool_calls, ["Bash"]);
        assert_eq!(s.tool_results.len(), 1);
        assert!(s.tool_results[0].0);
        assert!(s.tool_results[0].1.contains(CANARY_REASON));
    }

    #[test]
    fn only_the_deny_and_failure_canaries_are_required() {
        let required: Vec<&str> = Scenario::ALL
            .iter()
            .filter(|s| s.required())
            .map(|s| s.name())
            .collect();
        assert_eq!(required, ["deny", "hook_exit_1", "hook_timeout"]);
    }
}
