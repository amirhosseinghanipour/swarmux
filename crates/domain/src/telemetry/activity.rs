//! What swarmux's own jobs are doing, from their agent events: token flow,
//! tool calls, todos, environments, usage limits and the agent tree.

use std::collections::HashMap;

use super::environment::Environment;
use super::flow::TokenFlow;
use super::limits::{LimitState, RateLimit};
use super::pricing::TokenUsage;
use super::todo::Todo;
use super::tools::{ToolCall, ToolLedger};
use crate::{AgentEvent, JobId, JobProgress, JobStatus, Millis};

/// Tool calls kept per job.
pub const JOB_TOOL_CAP: usize = 1000;
/// Tool calls kept across jobs.
pub const SWARM_TOOL_CAP: usize = 500;

/// Telemetry of one job.
#[derive(Clone, Debug, PartialEq)]
pub struct JobActivity {
    pub flow: TokenFlow,
    pub tools: ToolLedger,
    /// The latest `TodoWrite` list of any of its agents.
    pub todos: Vec<Todo>,
    pub todos_at: Option<Millis>,
    pub environment: Option<Environment>,
    /// Latest usage report per actor, to skip repeats of one message.
    last_usage: HashMap<String, [u64; 4]>,
}

impl Default for JobActivity {
    fn default() -> JobActivity {
        JobActivity {
            flow: TokenFlow::default(),
            tools: ToolLedger::new(JOB_TOOL_CAP),
            todos: Vec::new(),
            todos_at: None,
            environment: None,
            last_usage: HashMap::new(),
        }
    }
}

impl JobActivity {
    /// Applies an event of `job` at `now`. Returns the usage it added, if any
    /// (repeats of the previous report of the same actor add nothing).
    pub fn observe(&mut self, job: JobId, ev: &AgentEvent, now: Millis) -> Option<TokenUsage> {
        match ev {
            AgentEvent::Usage {
                input,
                output,
                cache_read,
                cache_creation,
                actor,
            } => {
                let key = [*input, *output, *cache_read, *cache_creation];
                if self.last_usage.insert(actor.key().to_string(), key) == Some(key) {
                    return None;
                }
                let usage = TokenUsage {
                    input: *input,
                    output: *output,
                    cache_read: *cache_read,
                    cache_write_5m: *cache_creation,
                    cache_write_1h: 0,
                };
                self.flow.add(now, &usage);
                Some(usage)
            }
            AgentEvent::ToolUse {
                id,
                name,
                summary,
                actor,
            } => {
                self.tools
                    .started(tool_call(job, id, name, summary, actor.key(), now));
                None
            }
            AgentEvent::ToolResult {
                tool_use_id,
                is_error,
                ..
            } => {
                self.tools.finished(tool_use_id, *is_error, now);
                None
            }
            AgentEvent::Todos { todos, .. } => {
                self.todos = todos.clone();
                self.todos_at = Some(now);
                None
            }
            AgentEvent::Environment(env) => {
                self.environment = Some(env.as_ref().clone());
                None
            }
            _ => None,
        }
    }
}

fn tool_call(
    job: JobId,
    id: &str,
    name: &str,
    summary: &str,
    actor: &str,
    now: Millis,
) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        summary: summary.to_string(),
        actor: actor.to_string(),
        job: Some(job.0),
        started_at: now,
        ended_at: None,
        is_error: false,
    }
}

/// Telemetry across every job of this run, plus usage limits and session
/// environments (which outlive runs through storage).
#[derive(Clone, Debug, PartialEq)]
pub struct SwarmActivity {
    pub flow: TokenFlow,
    pub tools: ToolLedger,
    limits: Vec<LimitState>,
    /// Latest environment per Claude session, with when it was seen.
    environments: Vec<(Environment, Millis)>,
}

impl Default for SwarmActivity {
    fn default() -> SwarmActivity {
        SwarmActivity {
            flow: TokenFlow::default(),
            tools: ToolLedger::new(SWARM_TOOL_CAP),
            limits: Vec::new(),
            environments: Vec::new(),
        }
    }
}

impl SwarmActivity {
    /// Applies an event of `job`; `usage` is what the job's own activity added.
    pub fn observe(&mut self, job: JobId, ev: &AgentEvent, now: Millis, usage: Option<TokenUsage>) {
        if let Some(u) = usage {
            self.flow.add(now, &u);
        }
        match ev {
            AgentEvent::ToolUse {
                id,
                name,
                summary,
                actor,
            } => self
                .tools
                .started(tool_call(job, id, name, summary, actor.key(), now)),
            AgentEvent::ToolResult {
                tool_use_id,
                is_error,
                ..
            } => self.tools.finished(tool_use_id, *is_error, now),
            AgentEvent::RateLimit(limit) => self.record_limit(limit.clone(), now),
            AgentEvent::Environment(env) => self.record_environment(env.as_ref().clone(), now),
            _ => {}
        }
    }

    /// Takes a limit reading (also used to restore stored ones).
    pub fn record_limit(&mut self, limit: RateLimit, at: Millis) {
        match self
            .limits
            .iter_mut()
            .find(|s| s.limit.window == limit.window)
        {
            Some(s) if at >= s.observed_at => s.update(limit, at),
            Some(_) => {}
            None => self.limits.push(LimitState::new(limit, at)),
        }
    }

    /// Takes a session's environment (also used to restore stored ones).
    pub fn record_environment(&mut self, env: Environment, at: Millis) {
        match self
            .environments
            .iter_mut()
            .find(|(e, _)| e.session_id == env.session_id)
        {
            Some(slot) if at >= slot.1 => *slot = (env, at),
            Some(_) => {}
            None => self.environments.push((env, at)),
        }
    }

    /// Latest reading per window, shortest window first.
    pub fn limits(&self) -> Vec<&LimitState> {
        let mut out: Vec<&LimitState> = self.limits.iter().collect();
        out.sort_by_key(|s| {
            (
                super::limits::window_length(&s.limit.window).unwrap_or(Millis::MAX),
                s.limit.window.clone(),
            )
        });
        out
    }

    /// Environments, most recently seen first.
    pub fn environments(&self) -> Vec<&(Environment, Millis)> {
        let mut out: Vec<&(Environment, Millis)> = self.environments.iter().collect();
        out.sort_by_key(|e| std::cmp::Reverse(e.1));
        out
    }
}

/// One agent of a job and what it is doing.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentNode {
    /// `lead` or the subagent's tool_use id.
    pub key: String,
    pub label: String,
    pub model: Option<String>,
    pub running: bool,
    /// Its open tool call (`Bash cargo test`), if any.
    pub current_tool: Option<String>,
    pub tool_calls: u32,
    /// Tokens it read or wrote.
    pub tokens_seen: u64,
    pub children: Vec<AgentNode>,
}

/// The lead with its subagents as children, each with its current tool.
pub fn agent_tree(p: &JobProgress, a: &JobActivity) -> AgentNode {
    let current = |key: &str| {
        a.tools.current(key, Some(p.job_id.0)).map(|c| {
            if c.summary == c.name {
                c.name.clone()
            } else {
                format!("{} {}", c.name, c.summary)
            }
        })
    };
    let children = p
        .subagents
        .iter()
        .map(|s| AgentNode {
            key: s.id.clone(),
            label: s.kind.clone(),
            model: s.model.clone(),
            running: s.is_running(),
            current_tool: if s.is_running() { current(&s.id) } else { None },
            tool_calls: s.tool_calls,
            tokens_seen: s.tokens.seen,
            children: Vec::new(),
        })
        .collect();
    let running = p.status == JobStatus::Running;
    AgentNode {
        key: "lead".into(),
        label: p.lead_label.clone(),
        model: Some(p.model.clone()).filter(|m| !m.is_empty()),
        running,
        current_tool: if running { current("lead") } else { None },
        tool_calls: a.tools.calls_by("lead") as u32,
        tokens_seen: p.lead_tokens.seen,
        children,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Actor;

    fn usage(output: u64, actor: Actor) -> AgentEvent {
        AgentEvent::Usage {
            input: 1,
            output,
            cache_read: 8,
            cache_creation: 1,
            actor,
        }
    }

    fn tool(id: &str, name: &str, summary: &str, actor: Actor) -> AgentEvent {
        AgentEvent::ToolUse {
            id: id.into(),
            name: name.into(),
            summary: summary.into(),
            actor,
        }
    }

    fn result(id: &str, actor: Actor) -> AgentEvent {
        AgentEvent::ToolResult {
            tool_use_id: id.into(),
            is_error: false,
            preview: "ok".into(),
            actor,
        }
    }

    #[test]
    fn repeated_usage_counts_once_per_actor() {
        let mut a = JobActivity::default();
        let job = JobId(1);
        assert!(a.observe(job, &usage(5, Actor::Lead), 0).is_some());
        assert!(a.observe(job, &usage(5, Actor::Lead), 10).is_none());
        assert!(
            a.observe(job, &usage(5, Actor::Subagent("t".into())), 20)
                .is_some()
        );
        let s = a.flow.series(30);
        assert_eq!(s.iter().map(|b| b.output).sum::<u64>(), 10);
        assert_eq!(a.flow.cache_hit_ratio(30), Some(0.8));
    }

    #[test]
    fn todos_and_environment_keep_the_latest() {
        let mut a = JobActivity::default();
        let todos = vec![Todo {
            content: "x".into(),
            status: super::super::todo::TodoStatus::Pending,
            active_form: None,
        }];
        a.observe(
            JobId(1),
            &AgentEvent::Todos {
                todos: todos.clone(),
                actor: Actor::Lead,
            },
            7,
        );
        assert_eq!((a.todos.clone(), a.todos_at), (todos, Some(7)));
        let env = Environment {
            session_id: "s".into(),
            ..Environment::default()
        };
        a.observe(JobId(1), &AgentEvent::Environment(Box::new(env.clone())), 8);
        assert_eq!(a.environment, Some(env));
    }

    #[test]
    fn swarm_keeps_one_limit_per_window_and_env_per_session() {
        let mut s = SwarmActivity::default();
        let lim = |w: &str, u: f64| RateLimit {
            window: w.into(),
            status: "allowed".into(),
            utilization: Some(u),
            resets_at: Some(1_000_000),
            surpassed_threshold: None,
        };
        s.observe(
            JobId(1),
            &AgentEvent::RateLimit(lim("seven_day", 0.5)),
            10,
            None,
        );
        s.observe(
            JobId(1),
            &AgentEvent::RateLimit(lim("five_hour", 0.2)),
            20,
            None,
        );
        s.observe(
            JobId(2),
            &AgentEvent::RateLimit(lim("seven_day", 0.6)),
            30,
            None,
        );
        // An older stored reading doesn't replace a newer one.
        s.record_limit(lim("seven_day", 0.1), 5);
        let windows: Vec<(&str, Option<f64>)> = s
            .limits()
            .iter()
            .map(|l| (l.limit.window.as_str(), l.limit.utilization))
            .collect();
        assert_eq!(
            windows,
            [("five_hour", Some(0.2)), ("seven_day", Some(0.6))]
        );

        let env = |id: &str, v: &str| Environment {
            session_id: id.into(),
            model: v.into(),
            ..Environment::default()
        };
        s.record_environment(env("a", "x"), 1);
        s.record_environment(env("b", "x"), 2);
        s.record_environment(env("a", "y"), 3);
        let envs = s.environments();
        assert_eq!(envs.len(), 2);
        assert_eq!(
            (envs[0].0.session_id.as_str(), envs[0].0.model.as_str()),
            ("a", "y")
        );
    }

    #[test]
    fn swarm_flow_and_tools_across_jobs() {
        let mut s = SwarmActivity::default();
        let u = TokenUsage {
            output: 3,
            ..TokenUsage::default()
        };
        s.observe(JobId(1), &tool("t1", "Bash", "ls", Actor::Lead), 0, Some(u));
        s.observe(JobId(2), &tool("t2", "Read", "a.rs", Actor::Lead), 0, None);
        s.observe(JobId(1), &result("t1", Actor::Lead), 50, None);
        assert_eq!(s.tools.ticker(10).len(), 2);
        assert_eq!(s.tools.stats(None)[0].calls, 1);
        assert_eq!(s.flow.series(0).last().map(|b| b.output), Some(3));
    }

    #[test]
    fn tree_shows_current_tools_of_running_agents() {
        let mut p = JobProgress::new(JobId(1), "demo", "claude-opus-5-5");
        let mut a = JobActivity::default();
        let events = [
            tool("lead1", "Task", "explorer: map code", Actor::Lead),
            tool("sub1", "Grep", "fn main", Actor::Subagent("lead1".into())),
            tool("lead2", "Bash", "cargo test", Actor::Lead),
        ];
        for (i, ev) in events.iter().enumerate() {
            p.apply(ev, i as u32);
            a.observe(JobId(1), ev, i as Millis);
        }
        let tree = agent_tree(&p, &a);
        assert_eq!(tree.label, "lead");
        assert_eq!(tree.current_tool.as_deref(), Some("Bash cargo test"));
        assert_eq!(tree.tool_calls, 2);
        assert_eq!(tree.children.len(), 1);
        let child = &tree.children[0];
        assert_eq!((child.label.as_str(), child.running), ("explorer", true));
        assert_eq!(child.current_tool.as_deref(), Some("Grep fn main"));
        let ev = result("sub1", Actor::Subagent("lead1".into()));
        a.observe(JobId(1), &ev, 9);
        assert_eq!(agent_tree(&p, &a).children[0].current_tool, None);
    }
}
