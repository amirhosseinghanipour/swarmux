//! `Diagnoser` adapter: one cheap `claude -p --model haiku` call with no
//! tools and no session file, reading a stuck job's last steps and naming
//! what went wrong, through `--json-schema`.
//!
//! Only the watchdog's notes and the one-line steps the job's log already
//! shows are sent: no file contents.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use swarmux_application::{Diagnoser, Diagnosis, DiagnosisReply, DiagnosisRequest, DiagnosisSink};
use tokio::process::Command;
use tokio::runtime::Handle;

use super::router::run;

const SYSTEM: &str = "You diagnose coding agents that got stuck. From the goal and the agent's \
last steps, name the step where it went wrong and say in one sentence what it should do \
instead. Answer only through the structured output.";
/// Longest text kept from an answer, in characters.
const TEXT_MAX: usize = 240;
const SCHEMA: &str = r#"{"type":"object","properties":{"suspect":{"type":"string"},"instruction":{"type":"string"}},"required":["suspect","instruction"],"additionalProperties":false}"#;

/// Diagnoses stuck jobs with Haiku through the `claude` CLI.
pub struct HaikuDiagnoser {
    bin: PathBuf,
    rt: Handle,
    timeout: Duration,
    /// A neutral folder, so no project instructions load.
    cwd: PathBuf,
    bare: bool,
    env: Vec<(String, String)>,
}

impl HaikuDiagnoser {
    pub fn new(bin: PathBuf, rt: Handle) -> HaikuDiagnoser {
        HaikuDiagnoser {
            bin,
            rt,
            timeout: Duration::from_secs(90),
            cwd: std::env::temp_dir(),
            bare: false,
            env: Vec::new(),
        }
    }

    /// Adds `--bare` (only works with an API key).
    pub fn with_bare(mut self, bare: bool) -> HaikuDiagnoser {
        self.bare = bare;
        self
    }

    /// Extra environment for the child (used by tests).
    pub fn with_env(mut self, env: Vec<(String, String)>) -> HaikuDiagnoser {
        self.env = env;
        self
    }
}

impl Diagnoser for HaikuDiagnoser {
    fn diagnose(&self, request: DiagnosisRequest, sink: DiagnosisSink) {
        let mut cmd = Command::new(&self.bin);
        cmd.args(diagnose_args(self.bare))
            .envs(self.env.clone())
            .current_dir(&self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let timeout = self.timeout;
        let bin = self.bin.display().to_string();
        self.rt.spawn(async move {
            let prompt = diagnose_prompt(&request);
            let reply = match tokio::time::timeout(timeout, run(cmd, prompt, bin)).await {
                Ok(Ok(stdout)) => parse_diagnosis(&stdout),
                Ok(Err(why)) => failed(why),
                Err(_) => failed(format!("no answer within {} s", timeout.as_secs())),
            };
            sink(reply);
        });
    }
}

fn failed(why: String) -> DiagnosisReply {
    DiagnosisReply {
        result: Err(why),
        cost_usd: 0.0,
    }
}

/// `claude -p` flags for one tool-less Haiku answer in the diagnosis schema.
pub fn diagnose_args(bare: bool) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--model",
        "haiku",
        "--output-format",
        "json",
        "--json-schema",
        SCHEMA,
        "--tools",
        "",
        "--max-turns",
        "3",
        "--setting-sources",
        "project,local",
        "--strict-mcp-config",
        "--no-session-persistence",
        "--system-prompt",
        SYSTEM,
    ]
    .map(String::from)
    .to_vec();
    if bare {
        args.push("--bare".into());
    }
    args
}

/// What Haiku is asked.
pub fn diagnose_prompt(r: &DiagnosisRequest) -> String {
    let digest = json!({
        "goal": r.goal,
        "watchdog_saw": r.alarm,
        "already_tried": r.tried,
        "last_steps": r.steps,
    });
    format!(
        "A coding agent got stuck on the task below. Its last steps are listed oldest first \
         (tool calls, and ERROR lines for failed results).\n\n{}\n\n\
         Which step went wrong, and what should the agent do instead? Be specific and short.",
        serde_json::to_string_pretty(&digest).unwrap_or_default()
    )
}

/// Reads `claude --output-format json` output.
pub fn parse_diagnosis(stdout: &str) -> DiagnosisReply {
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
        Err(format!(
            "claude: {}",
            cut(text.lines().next().unwrap_or_default())
        ))
    } else {
        let answer = outer
            .get("structured_output")
            .cloned()
            .or_else(|| embedded(text));
        let field = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(cut);
        match answer {
            Some(v) => match (field(&v, "suspect"), field(&v, "instruction")) {
                (Some(suspect), Some(instruction))
                    if !suspect.is_empty() && !instruction.is_empty() =>
                {
                    Ok(Diagnosis {
                        suspect,
                        instruction,
                    })
                }
                _ => Err("haiku's answer is missing the suspect or the instruction".into()),
            },
            None => Err("haiku's answer has no JSON".into()),
        }
    };
    DiagnosisReply { result, cost_usd }
}

fn embedded(text: &str) -> Option<Value> {
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    serde_json::from_str(text.get(start..=end)?).ok()
}

fn cut(s: &str) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    match s.char_indices().nth(TEXT_MAX) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_domain::JobId;

    #[test]
    fn args_are_lean_tool_less_and_schema_bound() {
        let args = diagnose_args(true);
        let pair = |a: &str, b: &str| args.windows(2).any(|w| w[0] == a && w[1] == b);
        assert!(pair("--model", "haiku"));
        assert!(pair("--json-schema", SCHEMA));
        assert!(pair("--tools", ""));
        assert!(args.iter().any(|a| a == "--no-session-persistence"));
        assert_eq!(args.last().unwrap(), "--bare");
        assert!(!diagnose_args(false).iter().any(|a| a == "--bare"));
    }

    #[test]
    fn the_prompt_carries_only_the_digest() {
        let prompt = diagnose_prompt(&DiagnosisRequest {
            job: JobId(3),
            goal: "fix the parser".into(),
            alarm: "loop: Bash `cargo test` failed the same way 2 times".into(),
            steps: vec!["Bash cargo test".into(), "ERROR error[E0599]".into()],
            tried: vec!["nudge: waiting".into()],
        });
        assert!(prompt.contains("\"goal\": \"fix the parser\""), "{prompt}");
        assert!(prompt.contains("ERROR error[E0599]"));
        assert!(prompt.contains("nudge: waiting"));
    }

    #[test]
    fn answers_are_read_from_structured_output_or_text() {
        let out = r#"{"type":"result","is_error":false,"total_cost_usd":0.002,"result":"",
            "structured_output":{"suspect":"step 4 edits the fixture","instruction":"Leave the fixture alone."}}"#;
        let r = parse_diagnosis(out);
        assert_eq!(r.cost_usd, 0.002);
        assert_eq!(
            r.result.unwrap(),
            Diagnosis {
                suspect: "step 4 edits the fixture".into(),
                instruction: "Leave the fixture alone.".into()
            }
        );
        let out =
            r#"{"is_error":false,"result":"Here: {\"suspect\":\"a\",\"instruction\":\"b\"}"}"#;
        assert_eq!(parse_diagnosis(out).result.unwrap().suspect, "a");
    }

    #[test]
    fn bad_answers_are_errors() {
        assert!(parse_diagnosis("nope").result.is_err());
        let out = r#"{"is_error":true,"result":"Credit balance too low"}"#;
        assert!(
            parse_diagnosis(out)
                .result
                .unwrap_err()
                .contains("Credit balance")
        );
        let out = r#"{"is_error":false,"structured_output":{"suspect":""}}"#;
        assert!(parse_diagnosis(out).result.is_err());
    }
}
