//! The Board's MCP server.
//!
//! Each mission job is given `--mcp-config` registering `swarmux board-mcp
//! --job <id>`: a stdio MCP server that relays every tool call to the
//! running swarmux over the hook transport (one JSON line each way), so
//! the app stays the only writer of the store.

mod mcp;
mod tools;

use std::io::{self, BufReader};
use std::path::Path;

use serde_json::{Value, json};

use crate::hook::{ENV_PIPE, WireRequest, board_call};
pub use mcp::{ToolBackend, VERSIONS, handle_line, serve};
pub use tools::{
    SERVER_NAME, SERVER_NAME_PREFIX, TOOLS, ToolDef, list as tool_list, parse_request,
};

/// The `--mcp-config` JSON registering the board server for `job`. Claude
/// Code starts MCP servers with a trimmed environment, so the socket's
/// name is given explicitly.
pub fn mcp_config(exe: &Path, pipe: &str, job: i64) -> String {
    json!({
        "mcpServers": {
            SERVER_NAME: {
                "type": "stdio",
                "command": exe.to_string_lossy(),
                "args": ["board-mcp", "--job", job.to_string()],
                "env": { ENV_PIPE: pipe },
            }
        }
    })
    .to_string()
}

/// Reaches the app over the hook socket.
pub struct Relay {
    pipe: Option<String>,
    job: Option<i64>,
}

impl Relay {
    pub fn new(pipe: Option<String>, job: Option<i64>) -> Relay {
        Relay { pipe, job }
    }
}

impl ToolBackend for Relay {
    fn call(&self, tool: &str, arguments: &Value) -> Result<String, String> {
        let unreachable = || {
            "swarmux is not reachable from here, so the board is unavailable; carry on without it"
                .to_string()
        };
        let pipe = self
            .pipe
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .ok_or_else(unreachable)?;
        let request = WireRequest::board(tool, arguments.clone(), self.job);
        let reply = board_call(&request, pipe).ok_or_else(unreachable)?;
        let reply = reply.into_board();
        if reply.error {
            Err(reply.text)
        } else {
            Ok(reply.text)
        }
    }
}

/// `swarmux board-mcp --job <id>`: serves MCP on stdin and stdout.
pub fn board_mcp_main(job: Option<i64>) -> io::Result<()> {
    let relay = Relay::new(std::env::var(ENV_PIPE).ok(), job);
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    serve(BufReader::new(stdin.lock()), &mut stdout, &relay)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_registers_the_server_with_the_socket() {
        let v: Value =
            serde_json::from_str(&mcp_config(Path::new("/bin/swarmux"), "/tmp/x.sock", 12))
                .unwrap();
        let s = &v["mcpServers"]["swarmux-board"];
        assert_eq!(s["type"], "stdio");
        assert_eq!(s["command"], "/bin/swarmux");
        assert_eq!(s["args"], json!(["board-mcp", "--job", "12"]));
        assert_eq!(s["env"][ENV_PIPE], "/tmp/x.sock");
    }

    #[test]
    fn without_the_app_a_call_fails_with_a_readable_error() {
        let relay = Relay::new(None, Some(1));
        let err = relay.call("read", &json!({})).unwrap_err();
        assert!(err.contains("not reachable"));
        let gone = Relay::new(Some(crate::hook::endpoint_for(u32::MAX - 2)), Some(1));
        assert!(gone.call("read", &json!({})).is_err());
    }
}
