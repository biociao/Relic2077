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
    assert_eq!(tools.len(), 20);
    assert_eq!(tools[0]["name"], "relic_search");
    assert_eq!(tools[0]["annotations"]["readOnlyHint"], true);
    assert_eq!(tools[3]["annotations"]["readOnlyHint"], false);
    // The knowledge graph tools are read-only: traversing relations must never
    // be able to change the vault.
    for name in [
        "relic_find_similar",
        "relic_graph_neighbors",
        "relic_graph_path",
        "relic_explain_relation",
        "relic_graph_stats",
    ] {
        let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
        assert_eq!(tool["annotations"]["readOnlyHint"], true, "{name}");
    }
}

#[test]
fn traverses_the_knowledge_graph_through_mcp() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    // Unrelated memories, so the vector layer has a corpus to work with.
    for index in 0..5 {
        vault
            .create(
                &format!("Filler {index}"),
                "Unrelated note about alpine travel packing and waterproof boots.",
                "knowledge",
                vec!["life".into()],
                0.8,
                "test",
            )
            .unwrap();
    }
    let first = vault
        .create(
            "Chunking strategy for retrieval",
            "Use 512 token chunks for prose documents and validate the split against the corpus.",
            "knowledge",
            vec!["rag".into()],
            0.8,
            "test",
        )
        .unwrap();
    let second = vault
        .create(
            "Retrieval chunk size selection",
            "Chunk prose at 512 tokens so retrieval quality stays stable across short documents.",
            "knowledge",
            vec!["rag".into()],
            0.8,
            "test",
        )
        .unwrap();

    let output = run(
        vault,
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"relic_graph_neighbors","arguments":{"entry_id":first.meta.id,"depth":2}}}),
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"relic_graph_path","arguments":{"from_entry_id":first.meta.id,"to_entry_id":second.meta.id}}}),
            serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"relic_explain_relation","arguments":{"from_entry_id":first.meta.id,"to_entry_id":second.meta.id}}}),
            serde_json::json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"relic_graph_stats","arguments":{}}}),
            serde_json::json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"relic_find_similar","arguments":{"entry_id":first.meta.id,"top_k":3}}}),
            serde_json::json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"relic_search","arguments":{"query":"chunks prose tokens alpine","mode":"hybrid"}}}),
        ],
    );

    for response in &output {
        assert_eq!(response["result"]["isError"], false, "{response}");
    }
    let neighbors = output[0]["result"]["structuredContent"]["neighbors"]
        .as_array()
        .unwrap();
    assert!(
        neighbors
            .iter()
            .any(|hit| hit["id"] == second.meta.id.as_str()),
        "{neighbors:?}"
    );
    let path = &output[1]["result"]["structuredContent"]["path"];
    assert_eq!(path["nodes"].as_array().unwrap().len(), 2);
    let relations = output[2]["result"]["structuredContent"]["relations"]
        .as_array()
        .unwrap();
    assert!(
        relations
            .iter()
            .all(|relation| relation["evidence"].as_str().unwrap().len() > 10),
        "{relations:?}"
    );
    assert_eq!(
        output[3]["result"]["structuredContent"]["stats"]["nodes"],
        7
    );
    let similar = output[4]["result"]["structuredContent"]["results"]
        .as_array()
        .unwrap();
    assert_eq!(similar[0]["id"], second.meta.id.as_str());
    assert!(
        !similar[0]["shared_terms"].as_array().unwrap().is_empty(),
        "{similar:?}"
    );
    // The keyword layer finds nothing for this query; the hybrid layer still does.
    let hybrid = output[5]["result"]["structuredContent"]["results"]
        .as_array()
        .unwrap();
    assert!(!hybrid.is_empty());
    assert!(
        hybrid
            .iter()
            .any(|hit| hit["semantic_rank"].as_u64().is_some()),
        "{hybrid:?}"
    );
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

#[test]
fn capture_review_round_trip_uses_configured_source_and_is_idempotent() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let capture = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"relic_capture","arguments":{
        "event_id":"fix-1","project":"demo","session_id":"s1","title":"Restart fix",
        "context":"Search index missing","action":"Rebuild index","outcome":"Search restored","evidence":["Test passed"]
    }}});
    let mut output = Vec::new();
    serve_io_with_source_agent(
        vault.clone(),
        Cursor::new(format!("{capture}\n{capture}\n")),
        &mut output,
        "dsh",
    )
    .unwrap();
    let responses: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    let id = responses[0]["result"]["structuredContent"]["item"]["id"].clone();
    assert!(id.is_string(), "{responses:?}");
    assert_eq!(
        id,
        responses[1]["result"]["structuredContent"]["item"]["id"]
    );
    let call = |name: &str, arguments: Value| serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name,"arguments":arguments}});
    let review = serde_json::json!({"capture_id":id,"review":{"decision":"accept","reason":"Verified regression test", "knowledge":{
        "title":"Recover search after restart","content":"Rebuild the index from Markdown when missing.","kind":"lesson","confidence":0.9,"tags":["search"]
    }}});
    let output = run(
        vault.clone(),
        &[
            call("relic_process_captures", serde_json::json!({})),
            call("relic_get_capture", serde_json::json!({"capture_id":id})),
            call("relic_review_capture", review.clone()),
            call("relic_review_capture", review),
            call("relic_list_captures", serde_json::json!({})),
            call(
                "relic_get_capture",
                serde_json::json!({"capture_id":"../../escape"}),
            ),
        ],
    );
    for response in &output[..5] {
        assert_eq!(response["result"]["isError"], false, "{response}");
    }
    assert_eq!(
        output[1]["result"]["structuredContent"]["source"]["input"]["source_agent"],
        "dsh"
    );
    assert_eq!(
        output[2]["result"]["structuredContent"]["item"]["status"],
        "accepted"
    );
    assert_eq!(output[5]["result"]["isError"], true);
    let entries = vault.entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].meta.title, "Recover search after restart");
    assert_eq!(entries[0].meta.confidence, 0.9);
    assert_eq!(entries[0].meta.source_agents, vec!["dsh"]);
}
