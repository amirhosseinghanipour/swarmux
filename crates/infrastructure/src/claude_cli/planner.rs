//! `Planner` adapter: one `claude -p` call in plan mode, read-only tools
//! and no session file, answering with a plan and task graph through
//! `--json-schema`.
//!
//! The project's own instructions load (the plan has to fit the code it
//! is for), but the user's other settings and MCP servers do not, and
//! nothing can be written: plan mode and a read-only tool list.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use swarmux_application::{PlanReply, PlanRequest, PlanSink, Planner};
use swarmux_domain::plan::{PLAN_SCHEMA, parse_plan};
use tokio::process::Command;
use tokio::runtime::Handle;

use super::router::run;

const SYSTEM: &str = "You plan coding tasks before anyone builds them. You explore the \
repository with read-only tools, never change anything, and answer only through the \
structured output: a short plan and its task graph.";
/// Tools the planner may use: reading only.
const PLAN_TOOLS: &str = "Read,Grep,Glob";
/// Turns it gets to read the code and answer.
const PLAN_TURNS: &str = "20";
/// Longest a plan may take.
const TIMEOUT: Duration = Duration::from_secs(300);
/// Longest error text kept, in characters.
const TEXT_MAX: usize = 240;

/// Plans missions through the `claude` CLI.
pub struct ClaudePlanner {
    bin: PathBuf,
    rt: Handle,
    timeout: Duration,
    bare: bool,
    env: Vec<(String, String)>,
}

impl ClaudePlanner {
    pub fn new(bin: PathBuf, rt: Handle) -> ClaudePlanner {
        ClaudePlanner {
            bin,
            rt,
            timeout: TIMEOUT,
            bare: false,
            env: Vec::new(),
        }
    }

    /// Adds `--bare` (only works with an API key).
    pub fn with_bare(mut self, bare: bool) -> ClaudePlanner {
        self.bare = bare;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> ClaudePlanner {
        self.timeout = timeout;
        self
    }

    /// Extra environment for the child (used by tests).
    pub fn with_env(mut self, env: Vec<(String, String)>) -> ClaudePlanner {
        self.env = env;
        self
    }
}

impl Planner for ClaudePlanner {
    fn plan(&self, request: PlanRequest, sink: PlanSink) {
        let mut cmd = Command::new(&self.bin);
        cmd.args(plan_args(&request.model, self.bare))
            .envs(self.env.clone())
            .current_dir(&request.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let timeout = self.timeout;
        let bin = self.bin.display().to_string();
        self.rt.spawn(async move {
            let reply = match tokio::time::timeout(timeout, run(cmd, request.prompt, bin)).await {
                Ok(Ok(stdout)) => parse_reply(&stdout),
                Ok(Err(why)) => failed(why),
                Err(_) => failed(format!("no plan within {} s", timeout.as_secs())),
            };
            sink(reply);
        });
    }
}

fn failed(why: String) -> PlanReply {
    PlanReply {
        result: Err(why),
        cost_usd: 0.0,
    }
}

/// `claude -p` flags for one plan-mode run answering in the plan's schema.
pub fn plan_args(model: &str, bare: bool) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--model",
        model,
        "--output-format",
        "json",
        "--json-schema",
        PLAN_SCHEMA,
        "--permission-mode",
        "plan",
        "--tools",
        PLAN_TOOLS,
        "--max-turns",
        PLAN_TURNS,
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

/// Reads `claude --output-format json` output: the structured answer, or
/// the same JSON in the result text.
pub fn parse_reply(stdout: &str) -> PlanReply {
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
        match outer.get("structured_output") {
            Some(v) => parse_plan(&v.to_string()),
            None => parse_plan(text),
        }
    };
    PlanReply { result, cost_usd }
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
    use serde_json::json;

    fn plan_json() -> Value {
        json!({"summary": "two steps", "tasks": [
            {"id": "t1", "title": "Index", "files": ["src/index.rs"], "depends_on": [], "acceptance": "cargo test"},
            {"id": "t2", "title": "Query", "files": ["src/query/**"], "depends_on": ["t1"], "acceptance": ""}
        ]})
    }

    #[test]
    fn args_are_plan_mode_read_only_and_schema_bound() {
        let args = plan_args("sonnet", false);
        let pair = |a: &str, b: &str| args.windows(2).any(|w| w[0] == a && w[1] == b);
        assert_eq!(args[0], "-p");
        assert!(pair("--model", "sonnet"));
        assert!(pair("--permission-mode", "plan"));
        assert!(pair("--json-schema", PLAN_SCHEMA));
        assert!(pair("--tools", "Read,Grep,Glob"));
        assert!(args.contains(&"--no-session-persistence".to_string()));
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        assert!(!args.contains(&"--bare".to_string()));
        assert!(plan_args("sonnet", true).contains(&"--bare".to_string()));
        let schema: Value = serde_json::from_str(PLAN_SCHEMA).unwrap();
        assert_eq!(schema["properties"]["tasks"]["maxItems"], 30);
    }

    #[test]
    fn the_structured_output_is_the_plan() {
        let out = json!({"is_error": false, "total_cost_usd": 0.21, "result": "", "structured_output": plan_json()});
        let reply = parse_reply(&out.to_string());
        assert_eq!(reply.cost_usd, 0.21);
        let plan = reply.result.unwrap();
        assert_eq!(plan.tasks.len(), 2);
        assert_eq!(plan.tasks[1].depends_on, ["t1"]);
    }

    #[test]
    fn a_plan_in_the_result_text_is_read_too() {
        let text = format!("Here:\n```json\n{}\n```", plan_json());
        let out = json!({"is_error": false, "total_cost_usd": 0.1, "result": text});
        assert_eq!(parse_reply(&out.to_string()).result.unwrap().tasks.len(), 2);
    }

    #[test]
    fn bad_plans_and_errors_fail_but_keep_the_cost() {
        let cyclic = json!({"summary": "", "tasks": [
            {"id": "a", "title": "", "files": ["x"], "depends_on": ["b"], "acceptance": ""},
            {"id": "b", "title": "", "files": ["y"], "depends_on": ["a"], "acceptance": ""}
        ]});
        for (extra, why) in [
            (json!({"structured_output": cyclic}), "wait on each other"),
            (json!({"result": "no json here"}), "no JSON"),
            (
                json!({"structured_output": {"summary": "s", "tasks": []}}),
                "no tasks",
            ),
            (
                json!({"is_error": true, "result": "rate limited\nmore"}),
                "claude: rate limited",
            ),
        ] {
            let mut v = json!({"is_error": false, "total_cost_usd": 0.07, "result": ""});
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let reply = parse_reply(&v.to_string());
            let e = reply.result.unwrap_err();
            assert!(e.contains(why), "{e}");
            assert_eq!(reply.cost_usd, 0.07);
        }
        assert!(parse_reply("").result.is_err());
    }
}
