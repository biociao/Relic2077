use relic2077::mcp::{serve_io, serve_io_with_source_agent};
use relic2077::vault::Vault;
use serde_json::Value;
use std::io::Cursor;
use tempfile::tempdir;

fn run(vault: Vault, messages: &[Value]) -> Vec<Value> {
    let input = messages
        .iter()
        .map(|message| serde_json::to_string(message).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let mut output = Vec::new();
    serve_io(vault, Cursor::new(input), &mut output).unwrap();
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn negotiates_and_lists_annotated_tools() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let output = run(
        vault,
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),
            serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
        ],
    );
    assert_eq!(output.len(), 2);
    assert_eq!(output[0]["result"]["serverInfo"]["name"], "relic2077");
    let tools = output[1]["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 8);
    assert_eq!(tools[0]["name"], "relic_search");
    assert_eq!(tools[0]["annotations"]["readOnlyHint"], true);
    assert_eq!(tools[3]["annotations"]["readOnlyHint"], false);
}

#[test]
fn creates_and_searches_through_mcp() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let output = run(
        vault,
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"relic_create_entry","arguments":{"title":"Agent memory","content":"Knowledge survives agent switches.","tags":["memory"],"confidence":0.9,"source_agent":"test"}}}),
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"relic_search","arguments":{"query":"switches"}}}),
        ],
    );
    assert_eq!(output[0]["result"]["isError"], false);
    assert_eq!(
        output[1]["result"]["structuredContent"]["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn reports_tool_errors_without_crashing_server() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let output = run(
        vault,
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"relic_get_entry","arguments":{}}}),
        ],
    );
    assert_eq!(output[0]["result"]["isError"], true);
}

#[test]
fn records_the_configured_client_as_the_default_source_agent() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let input = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"relic_create_entry","arguments":{"title":"DSH memory","content":"Created from DeepSeek Harness."}}});
    let mut output = Vec::new();
    serve_io_with_source_agent(
        vault,
        Cursor::new(format!("{}\n", serde_json::to_string(&input).unwrap())),
        &mut output,
        "dsh",
    )
    .unwrap();
    let response: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        response["result"]["structuredContent"]["entry"]["meta"]["source_agents"][0],
        "dsh"
    );
}
