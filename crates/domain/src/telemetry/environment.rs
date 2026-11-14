//! What a Claude Code session runs with, from its `system/init` report.

use crate::json::{Json, quote};

/// An MCP server and its state ("connected", "pending", "failed", "needs-auth", …).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServer {
    pub name: String,
    pub status: String,
}

impl McpServer {
    pub fn is_up(&self) -> bool {
        self.status == "connected"
    }
}

/// A session's tools, servers, plugins and versions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Environment {
    pub session_id: String,
    pub claude_code_version: Option<String>,
    pub model: String,
    pub permission_mode: String,
    pub output_style: Option<String>,
    pub tools: Vec<String>,
    pub mcp_servers: Vec<McpServer>,
    pub plugins: Vec<String>,
    pub skills: Vec<String>,
    pub agents: Vec<String>,
    /// What the agent says it can do (`capabilities`), for feature detection.
    pub capabilities: Vec<String>,
}

impl Environment {
    /// (up, total) MCP servers.
    pub fn mcp_up(&self) -> (usize, usize) {
        let up = self.mcp_servers.iter().filter(|s| s.is_up()).count();
        (up, self.mcp_servers.len())
    }

    /// Short form for logs: `claude 2.1.316 · 86 tools · MCP 4/10 up · 15 plugins`.
    pub fn summary(&self) -> String {
        let (up, total) = self.mcp_up();
        let mut parts = Vec::new();
        if let Some(v) = &self.claude_code_version {
            parts.push(format!("claude {v}"));
        }
        parts.push(format!("{} tools", self.tools.len()));
        parts.push(format!("MCP {up}/{total} up"));
        parts.push(format!("{} plugins", self.plugins.len()));
        parts.push(format!("{} skills", self.skills.len()));
        parts.push(format!("{} agents", self.agents.len()));
        parts.join(" · ")
    }

    /// JSON for storage.
    pub fn encode(&self) -> String {
        let list = |items: &[String]| {
            let quoted: Vec<String> = items.iter().map(|s| quote(s)).collect();
            format!("[{}]", quoted.join(","))
        };
        let opt = |o: &Option<String>| o.as_deref().map_or("null".to_string(), quote);
        let servers: Vec<String> = self
            .mcp_servers
            .iter()
            .map(|s| {
                format!(
                    "{{\"name\":{},\"status\":{}}}",
                    quote(&s.name),
                    quote(&s.status)
                )
            })
            .collect();
        format!(
            "{{\"session_id\":{},\"version\":{},\"model\":{},\"permission_mode\":{},\
             \"output_style\":{},\"tools\":{},\"mcp_servers\":[{}],\"plugins\":{},\
             \"skills\":{},\"agents\":{},\"capabilities\":{}}}",
            quote(&self.session_id),
            opt(&self.claude_code_version),
            quote(&self.model),
            quote(&self.permission_mode),
            opt(&self.output_style),
            list(&self.tools),
            servers.join(","),
            list(&self.plugins),
            list(&self.skills),
            list(&self.agents),
            list(&self.capabilities),
        )
    }

    /// Reads what [`Environment::encode`] wrote.
    pub fn decode(text: &str) -> Option<Environment> {
        let v = Json::parse(text).ok()?;
        let s = |k: &str| v.get(k).and_then(Json::as_str).map(str::to_string);
        let list = |k: &str| -> Vec<String> {
            v.get(k)
                .and_then(Json::as_array)
                .unwrap_or_default()
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        };
        let mcp_servers = v
            .get("mcp_servers")
            .and_then(Json::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|x| {
                Some(McpServer {
                    name: x.get("name")?.as_str()?.to_string(),
                    status: x.get("status")?.as_str()?.to_string(),
                })
            })
            .collect();
        Some(Environment {
            session_id: s("session_id")?,
            claude_code_version: s("version"),
            model: s("model").unwrap_or_default(),
            permission_mode: s("permission_mode").unwrap_or_default(),
            output_style: s("output_style"),
            tools: list("tools"),
            mcp_servers,
            plugins: list("plugins"),
            skills: list("skills"),
            agents: list("agents"),
            capabilities: list("capabilities"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Environment {
        Environment {
            session_id: "s1".into(),
            claude_code_version: Some("2.1.316".into()),
            model: "claude-haiku-5-5".into(),
            permission_mode: "default".into(),
            output_style: None,
            tools: vec!["Read".into(), "Bash".into()],
            mcp_servers: vec![
                McpServer {
                    name: "docs \"x\"".into(),
                    status: "connected".into(),
                },
                McpServer {
                    name: "gmail".into(),
                    status: "needs-auth".into(),
                },
            ],
            plugins: vec!["claude-scholar".into()],
            skills: vec![],
            agents: vec!["explorer".into()],
            capabilities: vec!["interrupt_receipt_v1".into()],
        }
    }

    #[test]
    fn round_trips_through_storage() {
        let e = env();
        assert_eq!(Environment::decode(&e.encode()), Some(e));
        assert_eq!(Environment::decode("{}"), None);
    }

    #[test]
    fn mcp_counts_and_summary() {
        let e = env();
        assert_eq!(e.mcp_up(), (1, 2));
        assert_eq!(
            e.summary(),
            "claude 2.1.316 · 2 tools · MCP 1/2 up · 1 plugins · 0 skills · 1 agents"
        );
    }
}
