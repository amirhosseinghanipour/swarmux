//! `PromptRouter` adapter: one cheap `claude -p --model haiku` call with no
//! tools, no session file and none of the user's settings.

use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use serde_json::{Value, json};
use swarmux_application::ports::{PromptRouter, RouteAnswer, RouteReply, RouteRequest, RouteSink};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::runtime::Handle;

use super::contention::{self, is_config_contention};
use crate::process::{Stagger, random_below, spawn_tokio};

const SYSTEM: &str =
    "You route prompts to coding sessions. Answer with one JSON object and nothing else.";
const MODELS: [&str; 3] = ["haiku", "sonnet", "opus"];
const REASON_MAX: usize = 160;

/// Routes with Haiku through the `claude` CLI.
pub struct HaikuRouter {
    bin: PathBuf,
    rt: Handle,
    timeout: Duration,
    /// Where `claude` runs: a neutral folder, so no project instructions load.
    cwd: PathBuf,
    env: Vec<(String, String)>,
}

impl HaikuRouter {
    pub fn new(bin: PathBuf, rt: Handle) -> HaikuRouter {
        HaikuRouter {
            bin,
            rt,
            timeout: Duration::from_secs(60),
            cwd: std::env::temp_dir(),
            env: Vec::new(),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> HaikuRouter {
        self.timeout = timeout;
        self
    }

    /// Extra environment for the child (used by tests).
    pub fn with_env(mut self, env: Vec<(String, String)>) -> HaikuRouter {
        self.env = env;
        self
    }
}

impl PromptRouter for HaikuRouter {
    fn route(&self, request: RouteRequest, sink: RouteSink) {
        let mut cmd = Command::new(&self.bin);
        cmd.args(router_args())
            .envs(self.env.clone())
            .current_dir(&self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let timeout = self.timeout;
        let bin = self.bin.display().to_string();
        self.rt.spawn(async move {
            let run = run(cmd, router_prompt(&request), bin);
            let reply = match tokio::time::timeout(timeout, run).await {
                Ok(Ok(stdout)) => parse_route(&stdout, &request),
                Ok(Err(why)) => failed(why),
                Err(_) => failed(format!("no answer within {} s", timeout.as_secs())),
            };
            sink(reply);
        });
    }
}

/// Runs `claude` with `prompt` on stdin; its stdout, or why it failed.
/// Starts are spaced apart and repeated when another session held Claude
/// Code's config; dropping the future kills the process tree.
pub(crate) async fn run(mut cmd: Command, prompt: String, bin: String) -> Result<String, String> {
    let stagger = Stagger::shared();
    let mut retries = 0;
    loop {
        stagger.wait().await;
        let (stdout, status, stderr) = once(&mut cmd, &prompt, &bin).await?;
        let collided = !status.success() && stdout.is_empty() && is_config_contention(&stderr);
        if collided && retries < contention::RETRIES {
            let spread = random_below(Duration::from_millis(250));
            tokio::time::sleep(contention::backoff(retries, spread)).await;
            retries += 1;
            continue;
        }
        if !status.success() && !stdout.trim_start().starts_with('{') {
            let code = status.code().map_or("?".into(), |c| c.to_string());
            let detail = stderr.lines().next().unwrap_or_default();
            return Err(format!("claude exited with code {code} {detail}")
                .trim()
                .to_string());
        }
        return Ok(stdout);
    }
}

/// One start: stdout, exit status and stderr.
async fn once(
    cmd: &mut Command,
    prompt: &str,
    bin: &str,
) -> Result<(String, ExitStatus, String), String> {
    let (mut child, _tree) = spawn_tokio(cmd).map_err(|e| format!("cannot run {bin}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes()).await;
    }
    let mut stdout = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut stdout).await;
    }
    let mut stderr = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr).await;
    }
    let status = child.wait().await.map_err(|e| e.to_string())?;
    Ok((stdout, status, stderr))
}

fn failed(why: String) -> RouteReply {
    RouteReply {
        result: Err(why),
        cost_usd: 0.0,
    }
}

/// `claude -p` flags for one tool-less, settings-less Haiku turn.
pub fn router_args() -> Vec<String> {
    [
        "-p",
        "--model",
        "haiku",
        "--output-format",
        "json",
        "--tools",
        "",
        "--max-turns",
        "1",
        "--setting-sources",
        "project,local",
        "--strict-mcp-config",
        "--no-session-persistence",
        "--system-prompt",
        SYSTEM,
    ]
    .map(String::from)
    .to_vec()
}

/// What Haiku is asked.
pub fn router_prompt(request: &RouteRequest) -> String {
    let sessions: Vec<Value> = request
        .candidates
        .iter()
        .map(|c| {
            json!({
                "name": c.name,
                "folder": c.cwd,
                "model": c.model,
                "recent_prompts": c.recent,
            })
        })
        .collect();
    format!(
        "Pick the session that should take the prompt below.\n\n\
         Sessions (JSON):\n{}\n\n\
         Prompt:\n<<<\n{}\n>>>\n\n\
         Reply with one JSON object and nothing else:\n\
         {{\"session\": \"<one of the session names>\", \"model\": \"haiku\" | \"sonnet\" | \"opus\", \"reason\": \"<one short line>\"}}\n\
         model is the model the prompt needs: haiku for small edits and questions, \
         sonnet for most coding work, opus for hard or broad work.",
        serde_json::to_string_pretty(&sessions).unwrap_or_default(),
        request.prompt
    )
}

/// Reads `claude --output-format json` output and checks the pick.
pub fn parse_route(stdout: &str, request: &RouteRequest) -> RouteReply {
    let Ok(outer) = serde_json::from_str::<Value>(stdout.trim()) else {
        return failed("claude printed no JSON result".into());
    };
    let cost_usd = outer
        .get("total_cost_usd")
        .and_then(Value::as_f64)
        .filter(|c| c.is_finite() && *c >= 0.0)
        .unwrap_or(0.0);
    let text = outer.get("result").and_then(Value::as_str).unwrap_or("");
    let result = if outer.get("is_error").and_then(Value::as_bool) == Some(true) {
        Err(format!("claude: {}", first_line(text, REASON_MAX)))
    } else {
        read_answer(text, request)
    };
    RouteReply { result, cost_usd }
}

fn read_answer(text: &str, request: &RouteRequest) -> Result<RouteAnswer, String> {
    let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) else {
        return Err("haiku's answer has no JSON".into());
    };
    let answer: Value = serde_json::from_str(&text[start..=end])
        .map_err(|_| "haiku's answer is not valid JSON".to_string())?;
    let picked = answer
        .get("session")
        .and_then(Value::as_str)
        .ok_or("haiku's answer names no session")?
        .trim();
    let session = request
        .candidates
        .iter()
        .map(|c| c.name.as_str())
        .find(|n| *n == picked)
        .or_else(|| {
            request
                .candidates
                .iter()
                .map(|c| c.name.as_str())
                .find(|n| n.eq_ignore_ascii_case(picked))
        })
        .ok_or_else(|| format!("haiku picked unknown session {picked:?}"))?;
    let model = answer
        .get("model")
        .and_then(Value::as_str)
        .map(|m| m.trim().to_ascii_lowercase())
        .filter(|m| MODELS.contains(&m.as_str()));
    let reason = answer
        .get("reason")
        .and_then(Value::as_str)
        .map(|r| first_line(r, REASON_MAX))
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| "no reason given".into());
    Ok(RouteAnswer {
        session: session.to_string(),
        model,
        reason,
    })
}

fn first_line(s: &str, max: usize) -> String {
    let line = s.trim().lines().next().unwrap_or_default().trim();
    line.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_application::ports::RouteCandidate;

    fn request() -> RouteRequest {
        let c = |name: &str| RouteCandidate {
            name: name.into(),
            cwd: format!("/w/{name}"),
            model: "sonnet".into(),
            recent: vec!["add an endpoint".into()],
        };
        RouteRequest {
            prompt: "fix the login button css".into(),
            candidates: vec![c("api"), c("Web")],
        }
    }

    fn output(result: &str, is_error: bool) -> String {
        json!({"type":"result","is_error":is_error,"result":result,"total_cost_usd":0.000176})
            .to_string()
    }

    #[test]
    fn args_are_lean_and_tool_less() {
        let args = router_args();
        let pair = |a: &str, b: &str| args.windows(2).any(|w| w[0] == a && w[1] == b);
        assert!(pair("--model", "haiku"));
        assert!(pair("--output-format", "json"));
        assert!(pair("--tools", ""));
        assert!(pair("--max-turns", "1"));
        assert!(pair("--setting-sources", "project,local"));
        assert!(args.contains(&"--no-session-persistence".to_string()));
        assert_eq!(args[0], "-p");
    }

    #[test]
    fn prompt_lists_sessions_and_the_prompt() {
        let p = router_prompt(&request());
        assert!(p.contains("\"name\": \"api\""), "{p}");
        assert!(p.contains("\"folder\": \"/w/Web\""), "{p}");
        assert!(p.contains("add an endpoint"), "{p}");
        assert!(p.contains("<<<\nfix the login button css\n>>>"), "{p}");
    }

    #[test]
    fn a_valid_pick_is_read_with_cost() {
        let text = "```json\n{\"session\":\"web\",\"model\":\"Haiku\",\"reason\":\"front-end change\\nmore\"}\n```";
        let reply = parse_route(&output(text, false), &request());
        assert_eq!(reply.cost_usd, 0.000176);
        assert_eq!(
            reply.result.unwrap(),
            RouteAnswer {
                session: "Web".into(),
                model: Some("haiku".into()),
                reason: "front-end change".into(),
            }
        );
    }

    #[test]
    fn unknown_models_are_dropped() {
        let text = r#"{"session":"api","model":"gpt-5"}"#;
        let answer = parse_route(&output(text, false), &request())
            .result
            .unwrap();
        assert_eq!(
            (answer.model, answer.reason.as_str()),
            (None, "no reason given")
        );
    }

    #[test]
    fn bad_answers_fail_but_keep_the_cost() {
        for (text, why) in [
            ("I think api", "no JSON"),
            ("{not json}", "not valid JSON"),
            (r#"{"model":"haiku"}"#, "names no session"),
            (r#"{"session":"db"}"#, "unknown session \"db\""),
        ] {
            let reply = parse_route(&output(text, false), &request());
            let e = reply.result.unwrap_err();
            assert!(e.contains(why), "{text}: {e}");
            assert_eq!(reply.cost_usd, 0.000176);
        }
        let reply = parse_route(&output("rate limited", true), &request());
        assert_eq!(reply.result.unwrap_err(), "claude: rate limited");
        assert!(parse_route("", &request()).result.is_err());
    }
}
