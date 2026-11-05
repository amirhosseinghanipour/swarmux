//! `swarmux board-mcp` as Claude Code runs it: the real binary speaking MCP
//! on stdio, relaying to a real hook socket whose other end answers like
//! the app does.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use swarmux_application::{BoardInbox, BoardReply, BoardRequest, SharedPolicy};
use swarmux_domain::{GuardPolicy, Unmatched};
use swarmux_infrastructure::HookServer;
use swarmux_infrastructure::hook::endpoint_for;

const EXE: &str = env!("CARGO_BIN_EXE_swarmux");

struct Client {
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl Client {
    fn send(&mut self, message: impl ToString) -> Value {
        writeln!(self.stdin, "{}", message.to_string()).unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
}

#[test]
fn the_real_server_talks_mcp_and_relays_tool_calls_to_the_app() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let name = endpoint_for(std::process::id() ^ 0x5a00_0000);
    let policy = SharedPolicy::new(GuardPolicy::new(vec![], vec![], Unmatched::Pass));
    let srv = Arc::new(HookServer::start(rt.handle(), &name, policy).unwrap());

    // The app's side: refuses claims of locked/, takes everything else.
    let stop = Arc::new(AtomicBool::new(false));
    let app = {
        let (srv, stop) = (srv.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                for call in BoardInbox::poll(&*srv) {
                    let reply = match &call.request {
                        BoardRequest::Claim { paths, .. } if paths[0].starts_with("locked/") => {
                            BoardReply::failed(format!(
                                "rejected: {} is claimed by job 9",
                                paths[0]
                            ))
                        }
                        other => BoardReply {
                            text: format!("job {:?} did {other:?}", call.job.map(|j| j.0)),
                            error: false,
                        },
                    };
                    BoardInbox::respond(&*srv, call.id, reply);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        })
    };

    let mut child = Command::new(EXE)
        .args(["board-mcp", "--job", "12"])
        .env("SWARMUX_HOOK_PIPE", &name)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut client = Client {
        stdin: child.stdin.take().unwrap(),
        stdout: BufReader::new(child.stdout.take().unwrap()),
    };
    let init = client.send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "t", "version": "1"}}}));
    assert_eq!(init["result"]["serverInfo"]["name"], "swarmux-board");
    let list = client.send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 7);

    let ok = client.send(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"name": "fact", "arguments": {"text": "x", "evidence": "y"}}}));
    assert_eq!(ok["result"]["isError"], false);
    let text = ok["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("job Some(12)") && text.contains("Fact"),
        "{text}"
    );

    let refused = client.send(json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
        "params": {"name": "claim", "arguments": {"paths": ["locked/a.rs"]}}}));
    assert_eq!(refused["result"]["isError"], true);
    assert_eq!(
        refused["result"]["content"][0]["text"],
        "rejected: locked/a.rs is claimed by job 9"
    );

    let invalid = client.send(json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call",
        "params": {"name": "report", "arguments": {"status": "done"}}}));
    assert_eq!(invalid["result"]["isError"], true);
    assert!(
        invalid["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("summary is required")
    );

    let missing = client.send(json!({"jsonrpc": "2.0", "id": 6, "method": "nope"}));
    assert_eq!(missing["error"]["code"], -32601);

    // Malformed JSON, then the server still answers.
    let parse = client.send("{not json");
    assert_eq!(parse["error"]["code"], -32700);
    assert_eq!(
        client.send(json!({"jsonrpc": "2.0", "id": 7, "method": "ping"}))["result"],
        json!({})
    );

    drop(client.stdin);
    assert!(child.wait().unwrap().success(), "ends when stdin closes");
    stop.store(true, Ordering::Relaxed);
    app.join().unwrap();
}

#[test]
fn without_the_app_a_tool_call_is_an_error_not_a_hang() {
    let mut child = Command::new(EXE)
        .args(["board-mcp", "--job", "1"])
        .env_remove("SWARMUX_HOOK_PIPE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
               "params": {"name": "read", "arguments": {}}})
    )
    .unwrap();
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(reply["result"]["isError"], true);
}
