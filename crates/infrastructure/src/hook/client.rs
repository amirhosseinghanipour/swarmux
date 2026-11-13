//! `swarmux hook <event>`: the process Claude Code runs for each hook event.
//!
//! It forwards the event to the running app and prints the answer. When the
//! app can't be reached, what it prints for a tool call depends on the job:
//! unattended jobs get a denial (`SWARMUX_GUARD_FAIL=closed`), jobs a person
//! is watching get nothing, so Claude Code's own rules decide (`open`), or
//! its own question (`ask`). Every such fallback is written to a spool file
//! that the app puts in the ledger at its next start. Events Claude Code
//! doesn't wait for fall back silently.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use serde_json::json;
use swarmux_application::FailMode;
use swarmux_application::ports::FallbackRecord;
use swarmux_domain::ledger::{hex, sha256};
use swarmux_domain::{HookEvent, JobId};

use super::fallback::{ENV_FAIL, ENV_SPOOL, append, spool_file};
use super::protocol::{WireReply, WireRequest};
use super::transport::{ENV_JOB, ENV_PIPE, connect};

/// What a hook process knows about where it runs.
#[derive(Clone, Debug)]
pub struct HookEnv {
    pub pipe: Option<String>,
    pub job: Option<String>,
    /// What to do when the app can't be reached.
    pub fail: FailMode,
    /// Where fallbacks are written; `None` writes none.
    pub spool: Option<PathBuf>,
    /// The time, in milliseconds since the epoch.
    pub now: i64,
}

impl HookEnv {
    /// The environment of this process. A job started by a version that set
    /// no mode fails open, as it always did.
    pub fn from_process() -> HookEnv {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let spool = var(ENV_SPOOL).map(PathBuf::from).or_else(|| {
            let home = var(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
            Some(spool_file(&Path::new(&home).join(".swarmux")))
        });
        HookEnv {
            pipe: std::env::var(ENV_PIPE).ok(),
            job: std::env::var(ENV_JOB).ok(),
            fail: var(ENV_FAIL)
                .and_then(|m| FailMode::parse(&m))
                .unwrap_or(FailMode::Open),
            spool,
            now: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)),
        }
    }
}

/// Runs `swarmux hook <name>` with this process's stdin, stdout and
/// environment. An unknown `name` prints nothing.
pub fn hook_main(name: &str) {
    let Some(event) = HookEvent::parse(name) else {
        return;
    };
    let mut input = String::new();
    let read = std::io::stdin().read_to_string(&mut input).is_ok();
    let env = HookEnv::from_process();
    let out = if event == HookEvent::PreToolUse {
        // Input that can't be read is a failure like any other.
        pre_tool_in(if read { &input } else { "" }, &env)
    } else if read {
        run_hook(event, &input, env.pipe.as_deref(), env.job.as_deref())
    } else {
        None
    };
    if let Some(out) = out {
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{out}");
        let _ = stdout.flush();
    }
}

/// Runs the PreToolUse hook with this process's stdin, stdout and environment.
pub fn pre_tool_main() {
    hook_main(HookEvent::PreToolUse.as_str());
}

/// What the hook should print for `event` with hook input `stdin`, telling
/// the app at socket `pipe` about job `job`. `None` means print nothing.
///
/// Events Claude Code doesn't wait for are sent and forgotten.
pub fn run_hook(
    event: HookEvent,
    stdin: &str,
    pipe: Option<&str>,
    job: Option<&str>,
) -> Option<String> {
    if event == HookEvent::PreToolUse {
        return pre_tool(stdin, pipe, job);
    }
    let pipe = pipe.map(str::trim).filter(|p| !p.is_empty())?;
    let job_id = job.and_then(|j| j.trim().parse::<i64>().ok());
    let request = WireRequest::from_event_input(event, stdin, job_id)?;
    exchange(&request, pipe, event.is_blocking())?.hook_output()
}

/// What the hook should print for the PreToolUse input `stdin`, asking the
/// app at socket `pipe` about job `job`, when nothing is to be done about a
/// failure: `None` means print nothing.
pub fn pre_tool(stdin: &str, pipe: Option<&str>, job: Option<&str>) -> Option<String> {
    let pipe = pipe.map(str::trim).filter(|p| !p.is_empty())?;
    let job_id = job.and_then(|j| j.trim().parse::<i64>().ok());
    let request = WireRequest::from_hook_input(stdin, job_id)?;
    exchange(&request, pipe, true)?.hook_output()
}

/// The PreToolUse hook as a job's process runs it: `env` says what to do
/// when the app can't be reached. A process that isn't inside a swarmux job
/// (no socket and no job id) has nothing to guard and prints nothing.
pub fn pre_tool_in(stdin: &str, env: &HookEnv) -> Option<String> {
    let pipe = env.pipe.as_deref().map(str::trim).filter(|p| !p.is_empty());
    let job_id = env
        .job
        .as_deref()
        .and_then(|j| j.trim().parse::<i64>().ok());
    if pipe.is_none() && env.job.is_none() {
        return None;
    }
    let Some(pipe) = pipe else {
        return fall_back(stdin, env, job_id, "no socket");
    };
    let Some(mut request) = WireRequest::from_hook_input(stdin, job_id) else {
        return fall_back(stdin, env, job_id, "unreadable input");
    };
    request.fail = Some(env.fail.as_str().to_string());
    let mut line = match serde_json::to_string(&request) {
        Ok(l) => l,
        Err(_) => return fall_back(stdin, env, job_id, "unreadable input"),
    };
    line.push('\n');
    let mut conn = match connect(pipe) {
        Ok(c) => c,
        Err(_) => return fall_back(stdin, env, job_id, "connect failed"),
    };
    if conn.write_all(line.as_bytes()).is_err() || conn.flush().is_err() {
        return fall_back(stdin, env, job_id, "connect failed");
    }
    let mut answer = String::new();
    let reply = BufReader::new(conn)
        .read_line(&mut answer)
        .ok()
        .and_then(|_| serde_json::from_str::<WireReply>(&answer).ok());
    match reply {
        Some(reply) => reply.hook_output(),
        None => fall_back(stdin, env, job_id, "no answer"),
    }
}

/// What the hook prints when the app can't be reached, and the trace left
/// for the ledger.
fn fall_back(stdin: &str, env: &HookEnv, job: Option<i64>, reason: &str) -> Option<String> {
    let tool = serde_json::from_str::<serde_json::Value>(stdin)
        .ok()
        .and_then(|v| {
            v.get("tool_name")
                .and_then(|t| t.as_str().map(str::to_string))
        })
        .unwrap_or_default();
    let mode = match env.fail {
        FailMode::Closed => "deny",
        FailMode::Ask => "ask",
        FailMode::Open => "pass",
    };
    if let Some(spool) = &env.spool {
        append(
            spool,
            &FallbackRecord {
                ts: env.now,
                job: job.map(JobId),
                tool,
                input_sha256: hex(&sha256(stdin.as_bytes())),
                reason: reason.to_string(),
                mode: mode.to_string(),
                line_id: String::new(),
            },
        );
    }
    let why = match env.fail {
        FailMode::Closed => {
            format!("swarmux unreachable ({reason}); unattended jobs fail closed")
        }
        FailMode::Ask => format!("swarmux unreachable ({reason}); asking Claude Code to ask"),
        FailMode::Open => return None,
    };
    Some(
        json!({ "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": if env.fail == FailMode::Closed { "deny" } else { "ask" },
            "permissionDecisionReason": why,
        }})
        .to_string(),
    )
}

/// Sends `request` without waiting for an answer. `false` when the app
/// couldn't be reached.
pub(super) fn send(request: &WireRequest, pipe: &str) -> bool {
    let Ok(mut line) = serde_json::to_string(request) else {
        return false;
    };
    line.push('\n');
    let Ok(mut conn) = connect(pipe) else {
        return false;
    };
    conn.write_all(line.as_bytes()).is_ok() && conn.flush().is_ok()
}

/// Sends a board call and reads the app's reply. `None` when the app can't be
/// reached or doesn't answer.
pub fn board_call(request: &WireRequest, pipe: &str) -> Option<WireReply> {
    exchange(request, pipe, true)
}

/// Sends `request`; with `wait`, also reads the app's reply.
fn exchange(request: &WireRequest, pipe: &str, wait: bool) -> Option<WireReply> {
    let mut line = serde_json::to_string(request).ok()?;
    line.push('\n');
    let mut conn = connect(pipe).ok()?;
    conn.write_all(line.as_bytes()).ok()?;
    conn.flush().ok()?;
    if !wait {
        return None;
    }
    let mut answer = String::new();
    BufReader::new(conn).read_line(&mut answer).ok()?;
    serde_json::from_str(&answer).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INPUT: &str = r#"{"tool_name":"Bash","tool_input":{"command":"ls"},"cwd":"/w"}"#;

    #[test]
    fn no_pipe_means_no_output() {
        assert_eq!(pre_tool(INPUT, None, Some("1")), None);
        assert_eq!(pre_tool(INPUT, Some("  "), Some("1")), None);
        assert_eq!(run_hook(HookEvent::Stop, "{}", None, None), None);
    }

    #[test]
    fn unreachable_pipe_or_bad_input_means_no_output() {
        let pipe = super::super::transport::endpoint_for(u32::MAX - 1);
        assert_eq!(pre_tool(INPUT, Some(&pipe), None), None);
        assert_eq!(pre_tool("garbage", Some(&pipe), None), None);
        assert_eq!(run_hook(HookEvent::Stop, "{}", Some(&pipe), None), None);
        assert_eq!(
            run_hook(HookEvent::PostToolUse, "garbage", Some(&pipe), None),
            None
        );
    }

    fn env(fail: FailMode, spool: Option<PathBuf>) -> HookEnv {
        HookEnv {
            pipe: Some(super::super::transport::endpoint_for(u32::MAX - 2)),
            job: Some("7".into()),
            fail,
            spool,
            now: 1_700_000_000_000,
        }
    }

    fn decision_of(out: &str) -> (String, String) {
        let v: serde_json::Value = serde_json::from_str(out).unwrap();
        let h = &v["hookSpecificOutput"];
        (
            h["permissionDecision"].as_str().unwrap().to_string(),
            h["permissionDecisionReason"].as_str().unwrap().to_string(),
        )
    }

    #[test]
    fn closed_denies_when_the_app_is_unreachable_and_leaves_a_trace() {
        let dir = tempfile::tempdir().unwrap();
        let spool = spool_file(dir.path());
        let out = pre_tool_in(INPUT, &env(FailMode::Closed, Some(spool.clone()))).unwrap();
        let (decision, why) = decision_of(&out);
        assert_eq!(decision, "deny");
        assert!(why.contains("unattended jobs fail closed"), "{why}");
        assert!(why.contains("connect failed"), "{why}");
        let text = std::fs::read_to_string(&spool).unwrap();
        let line: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(line["tool"], "Bash");
        assert_eq!(line["mode"], "deny");
        assert_eq!(line["job"], 7);
        assert_eq!(line["reason"], "connect failed");
        assert_eq!(line["input_sha256"].as_str().unwrap().len(), 64);
        assert_eq!(line["ts"], 1_700_000_000_000_i64);
    }

    #[test]
    fn ask_asks_and_open_prints_nothing_but_both_leave_a_trace() {
        let dir = tempfile::tempdir().unwrap();
        let spool = spool_file(dir.path());
        let out = pre_tool_in(INPUT, &env(FailMode::Ask, Some(spool.clone()))).unwrap();
        assert_eq!(decision_of(&out).0, "ask");
        assert_eq!(
            pre_tool_in(INPUT, &env(FailMode::Open, Some(spool.clone()))),
            None
        );
        let lines = std::fs::read_to_string(&spool).unwrap();
        let modes: Vec<String> = lines
            .lines()
            .map(|l| {
                serde_json::from_str::<serde_json::Value>(l).unwrap()["mode"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(modes, ["ask", "pass"]);
    }

    #[test]
    fn input_that_cannot_be_read_and_a_missing_socket_fail_the_same_way() {
        let dir = tempfile::tempdir().unwrap();
        let spool = spool_file(dir.path());
        let out = pre_tool_in("garbage", &env(FailMode::Closed, Some(spool.clone()))).unwrap();
        assert!(decision_of(&out).1.contains("unreadable input"));
        let mut e = env(FailMode::Closed, Some(spool));
        e.pipe = None;
        let out = pre_tool_in(INPUT, &e).unwrap();
        assert!(decision_of(&out).1.contains("no socket"));
    }

    #[test]
    fn a_process_outside_a_job_guards_nothing() {
        let mut e = env(FailMode::Closed, None);
        e.pipe = None;
        e.job = None;
        assert_eq!(pre_tool_in(INPUT, &e), None);
    }
}
