//! Test stand-in for `claude` (built only with `--features fake-claude`).
//!
//! Env vars: FAKE_CLAUDE_FIXTURE (file printed to stdout), FAKE_CLAUDE_ECHO_FILE
//! (receives {"stdin","cwd","args","hook_pipe","job_id","added_dirs_claude_md"}
//! as JSON), FAKE_CLAUDE_STDERR, FAKE_CLAUDE_EXIT,
//! FAKE_CLAUDE_LINE_DELAY_MS (wait before each line of the fixture),
//! FAKE_CLAUDE_SLEEP_MS (wait before exiting) and FAKE_CLAUDE_DONE_FILE (written
//! after the wait, so tests can tell whether the process was killed).
//! FAKE_CLAUDE_ARGS_FILE gets one line of arguments per start.
//! FAKE_CLAUDE_STDIN_FILE gets one line per start: what that start was
//! told on stdin (the JSON user message), so a test can tell which start
//! received what.
//! FAKE_CLAUDE_START_FILE gets one line (nanoseconds since the epoch) per
//! start, FAKE_CLAUDE_PIDS_FILE one line per process id, and
//! FAKE_CLAUDE_GRANDCHILDREN=n starts n long-lived copies of this program.
//! FAKE_CLAUDE_CONTENTION=n with FAKE_CLAUDE_COUNTER_FILE makes the first n
//! starts fail as if another session held `.claude.json`.
//! FAKE_CLAUDE_HOOKS names a file of hook calls, one JSON line each
//! (`{"event":"PreToolUse","input":{...}}`, `{cwd}` in strings replaced by
//! the working folder), made through swarmux's hook client in order before
//! the fixture is printed; what each printed goes to FAKE_CLAUDE_HOOKS_OUT,
//! one line per call (`-` for nothing).
//! `--version` prints FAKE_CLAUDE_VERSION (default `2.1.323 (Claude Code)`), and
//! `hook canary <mode> [--log <file>]` runs swarmux's canary hook, so the
//! fake can stand in for both `claude` and the `swarmux` the canaries run.
//! FAKE_CLAUDE_HOOK_HOST=honor makes the fake behave as Claude Code does
//! toward a PreToolUse hook (what was seen on 2.1.323): it reads `--settings`,
//! makes the Bash call the prompt asks for (`touch <file>`, as the lead or as
//! a subagent), asks the hook, and makes the call unless the hook denied it
//! or failed with `onFailure: "block"` set. `=ignore` never asks the hook, as
//! when managed settings drop it.

use std::io::{Read, Write};

/// Appends one line to the file named by the environment variable `var`.
fn append_line(var: &str, line: &str) {
    if let Ok(path) = std::env::var(var) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        // One write, so lines from different processes never interleave.
        file.write_all(format!("{line}\n").as_bytes()).unwrap();
    }
}

/// Makes the hook calls the file at `path` lists, as Claude Code would.
fn run_hooks(path: &str) {
    let cwd = std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    let pipe = std::env::var("SWARMUX_HOOK_PIPE").ok();
    let job = std::env::var("SWARMUX_JOB_ID").ok();
    for line in std::fs::read_to_string(path).unwrap().lines() {
        if line.trim().is_empty() {
            continue;
        }
        let call: serde_json::Value = serde_json::from_str(&line.replace("{cwd}", &cwd)).unwrap();
        let event = swarmux_domain::HookEvent::parse(call["event"].as_str().unwrap()).unwrap();
        let mut input = call["input"].clone();
        input["cwd"] = cwd.clone().into();
        let out = swarmux_infrastructure::hook::run_hook(
            event,
            &input.to_string(),
            pipe.as_deref(),
            job.as_deref(),
        );
        append_line(
            "FAKE_CLAUDE_HOOKS_OUT",
            &out.unwrap_or_else(|| "-".into()).replace('\n', " "),
        );
    }
}

/// What the hook said, as Claude Code reads it.
enum HookSaid {
    Deny(String),
    /// Exit 1, a crash or no answer in time.
    Failed(String),
    /// Anything else lets the call through (no output, other output).
    Silent,
}

/// Runs the PreToolUse hook of the `--settings` file for one Bash call.
fn ask_hook(settings: &serde_json::Value, input: &str) -> HookSaid {
    let hook = &settings["hooks"]["PreToolUse"][0]["hooks"][0];
    let Some(command) = hook["command"].as_str() else {
        return HookSaid::Silent;
    };
    let args: Vec<String> = hook["args"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let timeout = hook["timeout"].as_u64().unwrap_or(600);
    let mut child = std::process::Command::new(command)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let started = std::time::Instant::now();
    loop {
        match child.try_wait().unwrap() {
            Some(status) => {
                let mut out = String::new();
                let _ = child.stdout.take().unwrap().read_to_string(&mut out);
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(out.trim()) {
                    let d = &v["hookSpecificOutput"];
                    if d["permissionDecision"] == "deny" {
                        return HookSaid::Deny(
                            d["permissionDecisionReason"]
                                .as_str()
                                .unwrap_or("")
                                .to_string(),
                        );
                    }
                }
                return if status.success() {
                    HookSaid::Silent
                } else {
                    HookSaid::Failed("failed".into())
                };
            }
            None if started.elapsed().as_secs() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return HookSaid::Failed("timed out".into());
            }
            None => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}

/// Plays one run of Claude Code against the hook (see the module docs).
fn hook_host(mode: &str) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let after = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let prompt = after("-p").unwrap_or_default();
    let file = prompt
        .split("touch ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or("canary.txt")
        .to_string();
    let settings: serde_json::Value = after("--settings")
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let by_subagent = prompt.contains("Task tool");
    let on_failure_block = settings["hooks"]["PreToolUse"][0]["hooks"][0]["onFailure"] == "block";
    let mut input = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": format!("touch {file}") },
    });
    if by_subagent {
        input["agent_id"] = "agent-1".into();
        input["agent_type"] = "general-purpose".into();
    }
    let said = if mode == "ignore" {
        HookSaid::Silent
    } else {
        ask_hook(&settings, &input.to_string())
    };
    let (blocked, why) = match said {
        HookSaid::Deny(reason) => (true, format!("PreToolUse:Bash hook error: {reason}")),
        HookSaid::Failed(how) if on_failure_block => (
            true,
            format!("PreToolUse:Bash hook error: {how}; blocking because onFailure is \"block\""),
        ),
        _ => (false, String::new()),
    };
    if !blocked {
        let _ = std::fs::write(&file, "");
    }
    let mut out = std::io::stdout().lock();
    let call = serde_json::json!({"type":"assistant","message":{"content":[
        {"type":"tool_use","id":"t1","name":"Bash","input":{"command":format!("touch {file}")}}]}});
    let back = if blocked {
        serde_json::json!({"type":"user","message":{"content":[
            {"type":"tool_result","tool_use_id":"t1","is_error":true,"content":why}]}})
    } else {
        serde_json::json!({"type":"user","message":{"content":[
            {"type":"tool_result","tool_use_id":"t1","is_error":false,"content":"(Bash completed with no output)"}]}})
    };
    let _ = writeln!(out, "{call}");
    let _ = writeln!(out, "{back}");
    let _ = writeln!(
        out,
        r#"{{"type":"result","subtype":"success","is_error":false}}"#
    );
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().is_some_and(|a| a == "--version") {
        let v =
            std::env::var("FAKE_CLAUDE_VERSION").unwrap_or_else(|_| "2.1.323 (Claude Code)".into());
        println!("{v}");
        return;
    }
    if argv.len() >= 2 && argv[0] == "hook" && argv[1] == "canary" {
        swarmux_infrastructure::hook::canary_main(&argv[2..]);
        return;
    }
    if let Ok(mode) = std::env::var("FAKE_CLAUDE_HOOK_HOST") {
        hook_host(&mode);
        return;
    }
    if std::env::var_os("FAKE_CLAUDE_START_FILE").is_some() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        append_line("FAKE_CLAUDE_START_FILE", &now.to_string());
    }
    append_line("FAKE_CLAUDE_PIDS_FILE", &std::process::id().to_string());
    if std::env::var_os("FAKE_CLAUDE_ARGS_FILE").is_some() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        append_line("FAKE_CLAUDE_ARGS_FILE", &args.join(" "));
    }
    if std::env::var_os("FAKE_CLAUDE_GRANDCHILD").is_some() {
        std::thread::sleep(std::time::Duration::from_secs(120));
        return;
    }
    if let Some(n) = std::env::var("FAKE_CLAUDE_GRANDCHILDREN")
        .ok()
        .and_then(|n| n.parse::<usize>().ok())
    {
        let exe = std::env::current_exe().unwrap();
        // They are meant to outlive this process unless something kills them.
        #[allow(clippy::zombie_processes)]
        for _ in 0..n {
            std::process::Command::new(&exe)
                .env("FAKE_CLAUDE_GRANDCHILD", "1")
                .env_remove("FAKE_CLAUDE_START_FILE")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
        }
    }
    if let (Some(n), Ok(path)) = (
        std::env::var("FAKE_CLAUDE_CONTENTION")
            .ok()
            .and_then(|n| n.parse::<usize>().ok()),
        std::env::var("FAKE_CLAUDE_COUNTER_FILE"),
    ) {
        let seen = std::fs::read_to_string(&path).map_or(0, |t| t.lines().count());
        append_line("FAKE_CLAUDE_COUNTER_FILE", "start");
        if seen < n {
            eprint!(
                "Claude configuration file at C:\\Users\\a\\.claude.json is corrupted: Unexpected end of JSON input"
            );
            std::process::exit(1);
        }
    }
    let mut stdin = String::new();
    let _ = std::io::stdin().read_to_string(&mut stdin);
    // One line per start: what this start was told. The JSON message has no
    // raw newline in it, so one line is one start.
    if std::env::var_os("FAKE_CLAUDE_STDIN_FILE").is_some() {
        append_line("FAKE_CLAUDE_STDIN_FILE", stdin.trim_end());
    }

    if let Ok(path) = std::env::var("FAKE_CLAUDE_ECHO_FILE") {
        let echo = serde_json::json!({
            "stdin": stdin,
            "cwd": std::env::current_dir().unwrap().to_string_lossy(),
            "args": std::env::args().skip(1).collect::<Vec<_>>(),
            "hook_pipe": std::env::var("SWARMUX_HOOK_PIPE").ok(),
            "job_id": std::env::var("SWARMUX_JOB_ID").ok(),
            "added_dirs_claude_md": std::env::var("CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD").ok(),
        });
        std::fs::write(path, echo.to_string()).unwrap();
    }
    if let Ok(path) = std::env::var("FAKE_CLAUDE_HOOKS") {
        run_hooks(&path);
    }
    if let Ok(path) = std::env::var("FAKE_CLAUDE_FIXTURE") {
        let text = std::fs::read_to_string(path).unwrap();
        match std::env::var("FAKE_CLAUDE_LINE_DELAY_MS")
            .ok()
            .and_then(|ms| ms.parse::<u64>().ok())
        {
            // One step at a time, as a real run streams them.
            Some(ms) => {
                let mut out = std::io::stdout().lock();
                for line in text.lines() {
                    std::thread::sleep(std::time::Duration::from_millis(ms));
                    writeln!(out, "{line}").unwrap();
                    out.flush().unwrap();
                }
            }
            None => print!("{text}"),
        }
    }
    if let Ok(msg) = std::env::var("FAKE_CLAUDE_STDERR") {
        eprint!("{msg}");
    }
    if let Some(ms) = std::env::var("FAKE_CLAUDE_SLEEP_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
    {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
    if let Ok(path) = std::env::var("FAKE_CLAUDE_DONE_FILE") {
        std::fs::write(path, "done").unwrap();
    }
    let code = std::env::var("FAKE_CLAUDE_EXIT")
        .ok()
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    std::process::exit(code);
}
