use crate::vault::{EntryPatch, Vault};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};

const PROTOCOL_VERSION: &str = "2025-06-18";
const INSTRUCTIONS: &str = "Relic is the user's local-first knowledge vault. Search before creating to avoid duplicates. Preserve entry IDs and history. Use confidence honestly. Prefer superseding obsolete knowledge over deleting it. Read tools are safe; write tools change Markdown files in the configured local vault.";

pub fn serve(vault: Vault) -> Result<()> {
    serve_with_source_agent(vault, "mcp-agent")
}

pub fn serve_with_source_agent(vault: Vault, source_agent: &str) -> Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    serve_io_with_source_agent(vault, stdin.lock(), stdout.lock(), source_agent)
}

pub fn serve_io<R: BufRead, W: Write>(vault: Vault, reader: R, writer: W) -> Result<()> {
    serve_io_with_source_agent(vault, reader, writer, "mcp-agent")
}

pub fn serve_io_with_source_agent<R: BufRead, W: Write>(
    vault: Vault,
    reader: R,
    mut writer: W,
    source_agent: &str,
) -> Result<()> {
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => {
                write_message(
                    &mut writer,
                    &error_response(Value::Null, -32700, &error.to_string()),
                )?;
                continue;
            }
        };
        if let Some(response) = handle_request(&vault, request, source_agent) {
            write_message(&mut writer, &response)?;
        }
    }
    Ok(())
}

fn write_message(writer: &mut impl Write, message: &Value) -> Result<()> {
    serde_json::to_writer(&mut *writer, message)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn handle_request(vault: &Vault, request: Value, source_agent: &str) -> Option<Value> {
    let id = request.get("id")?.clone();
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let result: Result<Value> = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "relic2077", "version": env!("CARGO_PKG_VERSION") },
            "instructions": INSTRUCTIONS
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => Ok(
            match call_tool(
                vault,
                request.get("params").unwrap_or(&Value::Null),
                source_agent,
            ) {
                Ok(result) => result,
                Err(error) => json!({
                    "content": [{ "type": "text", "text": format!("{error:#}") }],
                    "isError": true
                }),
            },
        ),
        _ => {
            return Some(error_response(
                id,
                -32601,
                &format!("method not found: {method}"),
            ));
        }
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => error_response(id, -32602, &format!("{error:#}")),
    })
}

fn call_tool(vault: &Vault, params: &Value, default_source_agent: &str) -> Result<Value> {
    let name = required_string(params, "name")?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let result = match name {
        "relic_search" => {
            let query = required_string(&arguments, "query")?;
            let top_k = arguments
                .get("top_k")
                .and_then(Value::as_u64)
                .unwrap_or(10)
                .clamp(1, 100) as usize;
            json!({ "results": vault.search(query, top_k)? })
        }
        "relic_get_entry" => {
            json!({ "entry": vault.get(required_string(&arguments, "entry_id")?)? })
        }
        "relic_list_entries" => {
            let kind = arguments.get("type").and_then(Value::as_str);
            let status = arguments.get("status").and_then(Value::as_str);
            let limit = arguments
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(50)
                .clamp(1, 200) as usize;
            let entries: Vec<_> = vault
                .entries()?
                .into_iter()
                .filter(|entry| {
                    kind.is_none_or(|value| entry.meta.kind == value)
                        && status.is_none_or(|value| entry.meta.status == value)
                })
                .take(limit)
                .collect();
            let count = entries.len();
            json!({ "entries": entries, "count": count })
        }
        "relic_create_entry" => {
            let title = required_string(&arguments, "title")?;
            let content = required_string(&arguments, "content")?;
            let kind = arguments
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("knowledge");
            let confidence = arguments
                .get("confidence")
                .and_then(Value::as_f64)
                .unwrap_or(0.7);
            let tags = string_array(&arguments, "tags")?;
            let source_agent = arguments
                .get("source_agent")
                .and_then(Value::as_str)
                .unwrap_or(default_source_agent);
            json!({ "entry": vault.create(title, content, kind, tags, confidence, source_agent)? })
        }
        "relic_update_entry" => {
            let id = required_string(&arguments, "entry_id")?;
            let patch = arguments.get("patch").context("missing 'patch'")?;
            let update = EntryPatch {
                title: optional_string(patch, "title")?,
                content: optional_string(patch, "content")?,
                kind: optional_string(patch, "type")?,
                status: optional_string(patch, "status")?,
                confidence: patch
                    .get("confidence")
                    .map(|v| v.as_f64().context("confidence must be a number"))
                    .transpose()?,
                tags: optional_string_array(patch, "tags")?,
                source_agents: optional_string_array(patch, "source_agents")?,
                links: optional_string_array(patch, "links")?,
            };
            json!({ "entry": vault.update(id, update)?, "reason": required_string(&arguments, "reason")? })
        }
        "relic_supersede_entry" => {
            let old_id = required_string(&arguments, "old_entry_id")?;
            let new_id = required_string(&arguments, "new_entry_id")?;
            let reason = required_string(&arguments, "reason")?;
            let (old, new) = vault.supersede(old_id, new_id)?;
            json!({ "old_entry": old.meta.id, "new_entry": new.meta.id, "reason": reason })
        }
        "relic_create_reflection" => {
            let period = arguments
                .get("period")
                .and_then(Value::as_str)
                .unwrap_or("weekly");
            let path = vault.create_reflection(period)?;
            json!({ "path": path, "period": period })
        }
        "relic_get_stats" => {
            let entries = vault.entries()?;
            let active = entries
                .iter()
                .filter(|entry| entry.meta.status == "active")
                .count();
            let fading = entries
                .iter()
                .filter(|entry| entry.meta.status == "fading")
                .count();
            let average_confidence = if entries.is_empty() {
                0.0
            } else {
                entries
                    .iter()
                    .map(|entry| entry.meta.confidence)
                    .sum::<f64>()
                    / entries.len() as f64
            };
            json!({ "entries": entries.len(), "active": active, "fading": fading, "average_confidence": average_confidence })
        }
        _ => anyhow::bail!("unknown tool '{name}'"),
    };
    let text = serde_json::to_string_pretty(&result)?;
    Ok(
        json!({ "content": [{ "type": "text", "text": text }], "structuredContent": result, "isError": false }),
    )
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
        .with_context(|| format!("missing or invalid '{key}'"))
}

fn optional_string(value: &Value, key: &str) -> Result<Option<String>> {
    value
        .get(key)
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .with_context(|| format!("'{key}' must be a string"))
        })
        .transpose()
}

fn string_array(value: &Value, key: &str) -> Result<Vec<String>> {
    Ok(optional_string_array(value, key)?.unwrap_or_default())
}

fn optional_string_array(value: &Value, key: &str) -> Result<Option<Vec<String>>> {
    value
        .get(key)
        .map(|v| {
            v.as_array()
                .context(format!("'{key}' must be an array"))?
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_owned)
                        .context(format!("'{key}' values must be strings"))
                })
                .collect()
        })
        .transpose()
}

fn error_response(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tools() -> Vec<Value> {
    vec![
        tool(
            "relic_search",
            "Search the local knowledge vault. Search before creating entries.",
            json!({"type":"object","properties":{"query":{"type":"string"},"top_k":{"type":"integer","minimum":1,"maximum":100,"default":10}},"required":["query"],"additionalProperties":false}),
            true,
            false,
            true,
        ),
        tool(
            "relic_get_entry",
            "Get one complete knowledge entry by its stable ID.",
            json!({"type":"object","properties":{"entry_id":{"type":"string"}},"required":["entry_id"],"additionalProperties":false}),
            true,
            false,
            true,
        ),
        tool(
            "relic_list_entries",
            "List recent entries, optionally filtered by type and status.",
            json!({"type":"object","properties":{"type":{"type":"string"},"status":{"enum":["active","fading","superseded","archived"]},"limit":{"type":"integer","minimum":1,"maximum":200,"default":50}},"additionalProperties":false}),
            true,
            false,
            true,
        ),
        tool(
            "relic_create_entry",
            "Create a durable Markdown knowledge entry after searching for duplicates.",
            json!({"type":"object","properties":{"title":{"type":"string"},"content":{"type":"string"},"type":{"enum":["knowledge","pattern","lesson","decision"]},"tags":{"type":"array","items":{"type":"string"}},"confidence":{"type":"number","minimum":0,"maximum":1},"source_agent":{"type":"string"}},"required":["title","content"],"additionalProperties":false}),
            false,
            false,
            false,
        ),
        tool(
            "relic_update_entry",
            "Update an existing entry. Supply a human-readable reason for the history.",
            json!({"type":"object","properties":{"entry_id":{"type":"string"},"patch":{"type":"object","properties":{"title":{"type":"string"},"content":{"type":"string"},"type":{"type":"string"},"status":{"enum":["active","fading","superseded","archived"]},"confidence":{"type":"number","minimum":0,"maximum":1},"tags":{"type":"array","items":{"type":"string"}},"source_agents":{"type":"array","items":{"type":"string"}},"links":{"type":"array","items":{"type":"string"}}},"additionalProperties":false},"reason":{"type":"string"}},"required":["entry_id","patch","reason"],"additionalProperties":false}),
            false,
            false,
            false,
        ),
        tool(
            "relic_supersede_entry",
            "Mark an obsolete entry as superseded by a newer entry while preserving history.",
            json!({"type":"object","properties":{"old_entry_id":{"type":"string"},"new_entry_id":{"type":"string"},"reason":{"type":"string"}},"required":["old_entry_id","new_entry_id","reason"],"additionalProperties":false}),
            false,
            false,
            false,
        ),
        tool(
            "relic_create_reflection",
            "Create a daily, weekly, or monthly reflection draft from recent activity.",
            json!({"type":"object","properties":{"period":{"enum":["daily","weekly","monthly"],"default":"weekly"}},"additionalProperties":false}),
            false,
            false,
            false,
        ),
        tool(
            "relic_get_stats",
            "Get local vault health and confidence statistics.",
            json!({"type":"object","properties":{},"additionalProperties":false}),
            true,
            false,
            true,
        ),
    ]
}

fn tool(
    name: &str,
    description: &str,
    input_schema: Value,
    read_only: bool,
    destructive: bool,
    idempotent: bool,
) -> Value {
    json!({ "name": name, "description": description, "inputSchema": input_schema, "annotations": {
        "readOnlyHint": read_only, "destructiveHint": destructive, "idempotentHint": idempotent, "openWorldHint": false
    }})
}
