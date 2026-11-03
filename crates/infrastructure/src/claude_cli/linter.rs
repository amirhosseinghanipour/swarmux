//! `MissionLinter` adapter: one cheap `claude -p --model haiku` call with
//! no tools and no session file, answering through `--json-schema`.
//!
//! `--bare` keeps the user's hooks, plugins and CLAUDE.md out of the call,
//! but it only authenticates with an API key, so it is used only when one
//! is configured.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use swarmux_application::{LintReply, LintRequest, LintSink, MissionLinter};
use swarmux_domain::mission::{MAX_QUESTIONS, Question};
use tokio::process::Command;
use tokio::runtime::Handle;

use super::router::run;

const SYSTEM: &str = "You review task specifications for coding agents before they run. \
You find what is missing or ambiguous and ask the fewest questions that would settle it. \
Answer only through the structured output.";
/// Longest question or reason kept, in characters.
const TEXT_MAX: usize = 240;
/// The structured output: up to three questions.
const SCHEMA: &str = r#"{"type":"object","properties":{"questions":{"type":"array","maxItems":3,"items":{"type":"object","properties":{"question":{"type":"string"},"why":{"type":"string"}},"required":["question","why"],"additionalProperties":false}}},"required":["questions"],"additionalProperties":false}"#;

/// Lints missions with Haiku through the `claude` CLI.
pub struct HaikuLinter {
    bin: PathBuf,
    rt: Handle,
    timeout: Duration,
    /// Where `claude` runs: a neutral folder, so no project instructions load.
    cwd: PathBuf,
    bare: bool,
    env: Vec<(String, String)>,
}

impl HaikuLinter {
    pub fn new(bin: PathBuf, rt: Handle) -> HaikuLinter {
        HaikuLinter {
            bin,
            rt,
            timeout: Duration::from_secs(90),
            cwd: std::env::temp_dir(),
            bare: false,
            env: Vec::new(),
        }
    }

    /// Adds `--bare` (only works with an API key).
    pub fn with_bare(mut self, bare: bool) -> HaikuLinter {
        self.bare = bare;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> HaikuLinter {
        self.timeout = timeout;
        self
    }

    /// Extra environment for the child (used by tests).
    pub fn with_env(mut self, env: Vec<(String, String)>) -> HaikuLinter {
        self.env = env;
        self
    }
}

impl MissionLinter for HaikuLinter {
    fn lint(&self, request: LintRequest, sink: LintSink) {
        let mut cmd = Command::new(&self.bin);
        cmd.args(linter_args(self.bare))
            .envs(self.env.clone())
            .current_dir(&self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let timeout = self.timeout;
        let bin = self.bin.display().to_string();
        self.rt.spawn(async move {
            let reply =
                match tokio::time::timeout(timeout, run(cmd, linter_prompt(&request), bin)).await {
                    Ok(Ok(stdout)) => parse_lint(&stdout),
                    Ok(Err(why)) => failed(why),
                    Err(_) => failed(format!("no answer within {} s", timeout.as_secs())),
                };
            sink(reply);
        });
    }
}

fn failed(why: String) -> LintReply {
    LintReply {
        result: Err(why),
        cost_usd: 0.0,
        source: "haiku".into(),
    }
}

/// `claude -p` flags for one tool-less Haiku answer in the lint's schema.
pub fn linter_args(bare: bool) -> Vec<String> {
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
        // The structured answer takes a turn of its own.
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
pub fn linter_prompt(request: &LintRequest) -> String {
    let c = &request.contract;
    let contract = json!({
        "title": c.title,
        "goal": c.goal,
        "acceptance_checks": c.acceptance.checks,
        "writable_paths": c.constraints.paths_rw,
        "protected_paths": c.constraints.protected,
        "budget_usd": c.budget.usd,
        "budget_minutes": c.budget.wall_minutes,
        "already_answered": c.clarifications.iter()
            .filter(|q| !q.answer.trim().is_empty())
            .map(|q| json!({"question": q.question, "answer": q.answer}))
            .collect::<Vec<_>>(),
    });
    let findings: Vec<&str> = request.findings.iter().map(|f| f.detail.as_str()).collect();
    format!(
        "A coding agent will be given the mission below and judged only by its acceptance checks.\n\n\
         Mission (JSON):\n{}\n\n\
         Already found by a local check:\n{}\n\n\
         Ask at most {MAX_QUESTIONS} questions whose answers would change what gets built or \
         how it is checked: ambiguous scope, a goal no check measures, missing acceptance \
         criteria, oracles the agent could edit. Ask none if the mission is clear. \
         Do not repeat what is already answered or already found.",
        serde_json::to_string_pretty(&contract).unwrap_or_default(),
        if findings.is_empty() {
            "- nothing".to_string()
        } else {
            findings
                .iter()
                .map(|f| format!("- {f}"))
                .collect::<Vec<_>>()
                .join("\n")
        },
    )
}

/// Reads `claude --output-format json` output: the structured answer, or
/// the same JSON in the result text.
pub fn parse_lint(stdout: &str) -> LintReply {
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
        match answer {
            Some(v) => questions(&v),
            None => Err("haiku's answer has no JSON".into()),
        }
    };
    LintReply {
        result,
        cost_usd,
        source: "haiku".into(),
    }
}

fn embedded(text: &str) -> Option<Value> {
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    serde_json::from_str(text.get(start..=end)?).ok()
}

fn questions(v: &Value) -> Result<Vec<Question>, String> {
    let list = v
        .get("questions")
        .and_then(Value::as_array)
        .ok_or("haiku's answer has no questions list")?;
    Ok(list
        .iter()
        .filter_map(|q| {
            let question = cut(q.get("question")?.as_str()?);
            let why = cut(q.get("why").and_then(Value::as_str).unwrap_or_default());
            (!question.is_empty()).then_some(Question { question, why })
        })
        .take(MAX_QUESTIONS)
        .collect())
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
    use swarmux_domain::mission::{Clarification, Contract, lint};

    fn request() -> LintRequest {
        let contract = Contract {
            title: "Faster app".into(),
            goal: "make the app faster".into(),
            clarifications: vec![Clarification {
                question: "Which screen?".into(),
                answer: "the search page".into(),
            }],
            ..Contract::default()
        };
        LintRequest {
            findings: lint(&contract),
            contract,
        }
    }

    #[test]
    fn args_are_lean_tool_less_and_schema_bound() {
        let args = linter_args(false);
        let pair = |a: &str, b: &str| args.windows(2).any(|w| w[0] == a && w[1] == b);
        assert_eq!(args[0], "-p");
        assert!(pair("--model", "haiku"));
        assert!(pair("--output-format", "json"));
        assert!(pair("--json-schema", SCHEMA));
        assert!(pair("--tools", ""));
        assert!(pair("--max-turns", "3"));
        assert!(args.contains(&"--no-session-persistence".to_string()));
        assert!(!args.contains(&"--bare".to_string()));
        assert!(linter_args(true).contains(&"--bare".to_string()));
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        assert_eq!(schema["properties"]["questions"]["maxItems"], 3);
    }

    #[test]
    fn the_prompt_carries_the_contract_findings_and_answers() {
        let p = linter_prompt(&request());
        assert!(p.contains("\"goal\": \"make the app faster\""), "{p}");
        assert!(p.contains("- no acceptance checks"), "{p}");
        assert!(p.contains("\"answer\": \"the search page\""), "{p}");
        assert!(p.contains("at most 3 questions"), "{p}");
    }

    fn output(extra: Value) -> String {
        let mut v = json!({"type":"result","is_error":false,"total_cost_usd":0.000485,"result":""});
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        v.to_string()
    }

    #[test]
    fn structured_output_is_read_first() {
        let out = output(json!({
            "structured_output": {"questions": [
                {"question": "  What does   faster mean? ", "why": "no metric"},
                {"question": "", "why": "dropped"},
                {"question": "Which check?", "why": "none given"},
                {"question": "Which repo?", "why": "unknown"},
                {"question": "A fourth?", "why": "too many"}
            ]},
            "result": "ignored"
        }));
        let reply = parse_lint(&out);
        assert_eq!(reply.cost_usd, 0.000485);
        let q = reply.result.unwrap();
        let asked: Vec<&str> = q.iter().map(|q| q.question.as_str()).collect();
        assert_eq!(
            asked,
            ["What does faster mean?", "Which check?", "Which repo?"]
        );
        assert_eq!(q[0].why, "no metric");
    }

    #[test]
    fn the_result_text_is_read_when_there_is_no_structured_output() {
        let text = "```json\n{\"questions\":[{\"question\":\"Which corpus?\",\"why\":\"x\"}]}\n```";
        let reply = parse_lint(&output(json!({ "result": text })));
        assert_eq!(reply.result.unwrap()[0].question, "Which corpus?");
        let none = parse_lint(&output(json!({"structured_output": {"questions": []}})));
        assert_eq!(none.result.unwrap(), []);
    }

    #[test]
    fn bad_answers_fail_but_keep_the_cost() {
        for (extra, why) in [
            (json!({"result": "no json here"}), "no JSON"),
            (
                json!({"structured_output": {"qs": []}}),
                "no questions list",
            ),
            (
                json!({"is_error": true, "result": "rate limited\nmore"}),
                "claude: rate limited",
            ),
        ] {
            let reply = parse_lint(&output(extra));
            let e = reply.result.unwrap_err();
            assert!(e.contains(why), "{e}");
            assert_eq!(reply.cost_usd, 0.000485);
        }
        assert!(parse_lint("").result.is_err());
    }
}
