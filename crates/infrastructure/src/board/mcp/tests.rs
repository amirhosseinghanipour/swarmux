use std::cell::RefCell;
use std::io::Cursor;

use super::*;

/// Records calls; rejects claims of `locked/`.
#[derive(Default)]
struct Fake {
    calls: RefCell<Vec<(String, Value)>>,
}

impl ToolBackend for Fake {
    fn call(&self, tool: &str, arguments: &Value) -> Result<String, String> {
        self.calls
            .borrow_mut()
            .push((tool.to_string(), arguments.clone()));
        if tool == "claim" && arguments.to_string().contains("locked/") {
            return Err("rejected: locked/ is claimed by job 7".into());
        }
        Ok(format!("{tool} done"))
    }
}

fn exchange(lines: &[Value]) -> Vec<Value> {
    exchange_raw(
        &lines
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn exchange_raw(input: &str) -> Vec<Value> {
    let mut out = Vec::new();
    serve(Cursor::new(input.to_string()), &mut out, &Fake::default()).unwrap();
    String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

#[test]
fn the_handshake_negotiates_the_version_and_names_the_server() {
    let replies = exchange(&[
        request(
            1,
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "claude-code", "version": "2"}}),
        ),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        request(2, "ping", json!({})),
    ]);
    assert_eq!(replies.len(), 2, "a notification gets no reply");
    let init = &replies[0];
    assert_eq!(init["jsonrpc"], "2.0");
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "swarmux-board");
    assert!(init["result"]["serverInfo"]["version"].is_string());
    assert!(init["result"]["capabilities"]["tools"].is_object());
    assert_eq!(replies[1], json!({"jsonrpc": "2.0", "id": 2, "result": {}}));
}

#[test]
fn an_unknown_version_is_answered_with_the_servers_newest() {
    let r = exchange(&[request(
        1,
        "initialize",
        json!({"protocolVersion": "1999-01-01"}),
    )]);
    assert_eq!(r[0]["result"]["protocolVersion"], VERSIONS[0]);
    let r = exchange(&[request(1, "initialize", json!({}))]);
    assert_eq!(r[0]["result"]["protocolVersion"], VERSIONS[0]);
    let r = exchange(&[request(
        1,
        "initialize",
        json!({"protocolVersion": "2024-11-05"}),
    )]);
    assert_eq!(r[0]["result"]["protocolVersion"], "2024-11-05");
}

#[test]
fn tools_list_returns_every_tool_with_a_json_schema() {
    let r = exchange(&[request(3, "tools/list", json!({}))]);
    let tools = r[0]["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "claim",
            "report",
            "fact",
            "decision",
            "ask_human",
            "schedule_followup",
            "read"
        ]
    );
    for t in tools {
        assert!(t["description"].as_str().is_some_and(|d| !d.is_empty()));
        assert_eq!(t["inputSchema"]["type"], "object");
        assert!(t["inputSchema"]["properties"].is_object());
    }
    let claim = &tools[0]["inputSchema"];
    assert_eq!(claim["required"], json!(["paths"]));
    assert_eq!(claim["properties"]["paths"]["type"], "array");
}

#[test]
fn a_successful_call_returns_text_content() {
    let r = exchange(&[request(
        4,
        "tools/call",
        json!({"name": "fact", "arguments": {"text": "x"}}),
    )]);
    assert_eq!(
        r[0]["result"],
        json!({"content": [{"type": "text", "text": "fact done"}], "isError": false})
    );
}

#[test]
fn a_rejected_call_is_a_result_with_is_error() {
    let r = exchange(&[request(
        5,
        "tools/call",
        json!({"name": "claim", "arguments": {"paths": ["locked/a"]}}),
    )]);
    assert!(r[0].get("error").is_none());
    assert_eq!(r[0]["result"]["isError"], true);
    assert_eq!(
        r[0]["result"]["content"][0]["text"],
        "rejected: locked/ is claimed by job 7"
    );
}

#[test]
fn an_unknown_tool_or_bad_arguments_is_invalid_params() {
    let r = exchange(&[
        request(
            6,
            "tools/call",
            json!({"name": "teleport", "arguments": {}}),
        ),
        request(7, "tools/call", json!({"arguments": {}})),
        request(8, "tools/call", json!({"name": "fact", "arguments": [1]})),
    ]);
    for reply in &r {
        assert_eq!(reply["error"]["code"], -32602);
    }
    assert_eq!(r[0]["id"], 6);
}

#[test]
fn an_unknown_method_is_method_not_found() {
    let r = exchange(&[request(9, "resources/list", json!({}))]);
    assert_eq!(r[0]["error"]["code"], -32601);
    assert_eq!(r[0]["id"], 9);
    // An unknown notification is ignored.
    assert!(exchange(&[json!({"jsonrpc": "2.0", "method": "nope"})]).is_empty());
}

#[test]
fn malformed_json_is_a_parse_error_and_the_server_goes_on() {
    let r = exchange_raw(
        "{\"jsonrpc\": \"2.0\", \"id\": 1, \"meth\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}",
    );
    assert_eq!(r[0]["error"]["code"], -32700);
    assert_eq!(r[0]["id"], Value::Null);
    assert_eq!(r[1]["id"], 2);
}

#[test]
fn things_that_are_not_requests_are_invalid_requests() {
    let r = exchange_raw("[1,2]\n42\n{\"id\":1}\n{\"id\":[1],\"method\":\"ping\"}\n");
    assert_eq!(r.len(), 4);
    for reply in &r {
        assert_eq!(reply["error"]["code"], -32600);
    }
}

#[test]
fn blank_lines_are_skipped_and_ids_keep_their_type() {
    let r = exchange_raw("\n\n{\"jsonrpc\":\"2.0\",\"id\":\"abc\",\"method\":\"ping\"}\n");
    assert_eq!(r[0]["id"], "abc");
}
