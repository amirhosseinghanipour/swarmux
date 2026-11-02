/// Who did something inside a job: the lead session or one of its subagents.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Actor {
    Lead,
    /// Identified by the `tool_use` id of the Task/Agent call that started it.
    Subagent(String),
}

impl Actor {
    /// Stable key for storage: `"lead"` or the tool_use id.
    pub fn key(&self) -> &str {
        match self {
            Actor::Lead => "lead",
            Actor::Subagent(id) => id,
        }
    }
}

/// Something an agent did, independent of how Claude Code reported it.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentEvent {
    Init {
        session_id: String,
        model: String,
        cwd: String,
        permission_mode: String,
    },
    Text {
        text: String,
        actor: Actor,
    },
    ToolUse {
        id: String,
        name: String,
        summary: String,
        actor: Actor,
    },
    ToolResult {
        tool_use_id: String,
        is_error: bool,
        preview: String,
        actor: Actor,
    },
    Usage {
        input: u64,
        output: u64,
        cache_read: u64,
        cache_creation: u64,
        actor: Actor,
    },
    Result {
        is_error: bool,
        subtype: String,
        cost_usd: f64,
        duration_ms: u64,
        num_turns: u32,
        tokens_in: u64,
        tokens_out: u64,
        cache_read: u64,
        context_window: Option<u64>,
        subagents_spawned: u32,
    },
    /// Known noise (hook progress, rate-limit info, …).
    Ignored {
        kind: String,
    },
    /// A report type this version doesn't know yet; kept raw.
    Unknown {
        kind: String,
        raw: String,
    },
    /// Output that wasn't a valid report at all.
    Malformed {
        raw: String,
    },
}

impl AgentEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            AgentEvent::Init { .. } => "init",
            AgentEvent::Text { .. } => "text",
            AgentEvent::ToolUse { .. } => "tool_use",
            AgentEvent::ToolResult { .. } => "tool_result",
            AgentEvent::Usage { .. } => "usage",
            AgentEvent::Result { .. } => "result",
            AgentEvent::Ignored { .. } => "ignored",
            AgentEvent::Unknown { .. } => "unknown",
            AgentEvent::Malformed { .. } => "malformed",
        }
    }

    pub fn actor(&self) -> &Actor {
        match self {
            AgentEvent::Text { actor, .. }
            | AgentEvent::ToolUse { actor, .. }
            | AgentEvent::ToolResult { actor, .. }
            | AgentEvent::Usage { actor, .. } => actor,
            _ => &Actor::Lead,
        }
    }

    /// True for the Task/Agent tool call that starts a subagent.
    pub fn starts_subagent(&self) -> bool {
        matches!(self, AgentEvent::ToolUse { name, .. } if name == "Task" || name == "Agent")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_keys() {
        assert_eq!(Actor::Lead.key(), "lead");
        assert_eq!(Actor::Subagent("toolu_A".into()).key(), "toolu_A");
    }

    #[test]
    fn result_and_init_belong_to_lead() {
        let ev = AgentEvent::Ignored { kind: "x".into() };
        assert_eq!(ev.actor(), &Actor::Lead);
    }

    #[test]
    fn task_tool_starts_a_subagent() {
        let ev = AgentEvent::ToolUse {
            id: "t".into(),
            name: "Task".into(),
            summary: "explorer: map".into(),
            actor: Actor::Lead,
        };
        assert!(ev.starts_subagent());
    }
}
