use crate::vault::{EntryPatch, Vault};
use anyhow::{Context, Result, bail};
use async_stream::stream;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, ORIGIN, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

const PROTOCOL_VERSION: &str = "2025-06-18";
const INSTRUCTIONS: &str = "Relic is the user's local-first knowledge vault. Search before creating to avoid duplicates. Preserve entry IDs and history. Use confidence honestly. Prefer superseding obsolete knowledge over deleting it. Read tools are safe; write tools change Markdown files in the configured local vault.";
const SESSION_HEADER: &str = "mcp-session-id";
const SESSION_IDLE_TTL: Duration = Duration::from_secs(30 * 60);
const OAUTH_TITLE: &str = "Relic2077 OAuth 2.1 Authorization Server";

#[derive(Clone)]
struct HttpState {
    vault: Arc<Mutex<Vault>>,
    source_agent: Arc<str>,
    bearer_token: Option<Arc<str>>,
    allowed_origins: Arc<[String]>,
    sessions: Arc<Mutex<HashMap<String, Instant>>>,
}

pub fn serve_http(
    vault: Vault,
    bind: SocketAddr,
    source_agent: &str,
    bearer_token: Option<String>,
    allowed_origins: Vec<String>,
) -> Result<()> {
    if !bind.ip().is_loopback() && bearer_token.is_none() {
        bail!("refusing to expose MCP beyond localhost without --bearer-token-env");
    }
    let app = http_router(
        vault,
        bind.port(),
        source_agent,
        bearer_token,
        allowed_origins,
    );
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(bind).await?;
        eprintln!("Relic MCP listening on http://{bind}/mcp");
        axum::serve(listener, app)
            .await
            .context("HTTP MCP server failed")
    })
}

pub fn http_router(
    vault: Vault,
    port: u16,
    source_agent: &str,
    bearer_token: Option<String>,
    mut allowed_origins: Vec<String>,
) -> Router {
    allowed_origins.extend([
        format!("http://localhost:{port}"),
        format!("http://127.0.0.1:{port}"),
        format!("http://[::1]:{port}"),
    ]);
    allowed_origins.sort();
    allowed_origins.dedup();

    let state = HttpState {
        vault: Arc::new(Mutex::new(vault)),
        source_agent: Arc::from(source_agent),
        bearer_token: bearer_token.map(Arc::from),
        allowed_origins: allowed_origins.into(),
        sessions: Arc::new(Mutex::new(HashMap::new())),
    };
    Router::new()
        .route("/mcp", get(http_get).post(http_post))
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth_metadata),
        )
        .with_state(state)
}

async fn http_get(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    if let Err(response) = validate_endpoint_headers(&state, &headers) {
        return *response;
    }
    if let Err(response) = validate_session(&state, &headers) {
        return *response;
    }
    // A long-lived Server-Sent Events channel for server-initiated messages.
    // The channel stays open; a keep-alive comment is flushed periodically so
    // clients can detect a still-live connection. Responses to tool calls are
    // delivered over the POST response stream instead.
    let channel = stream! {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            yield Ok::<Event, std::convert::Infallible>(Event::default());
        }
    };
    Sse::new(channel)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn http_post(State(state): State<HttpState>, headers: HeaderMap, body: Bytes) -> Response {
    if let Err(response) = validate_http_headers(&state, &headers) {
        return *response;
    }
    if let Err(response) = validate_session(&state, &headers) {
        return *response;
    }
    let request: Value = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                error_response(Value::Null, -32700, &error.to_string()),
            );
        }
    };
    let responses = {
        let vault = match state.vault.lock() {
            Ok(vault) => vault,
            Err(_) => {
                return json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_response(Value::Null, -32603, "vault lock poisoned"),
                );
            }
        };
        process_request(&vault, &request, &state.source_agent)
    };
    let session_id = if messages(request.clone()).any(is_initialize) {
        let session_id = Uuid::new_v4().simple().to_string();
        if let Ok(mut sessions) = state.sessions.lock() {
            sessions.insert(session_id.clone(), Instant::now());
        }
        Some(session_id)
    } else {
        None
    };
    let wants_sse = headers
        .get(ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.contains("text/event-stream") && !value.contains("application/json")
        });
    if responses.iter().all(Option::is_none) {
        // A request containing only notifications is acknowledged, not answered.
        let response = StatusCode::ACCEPTED.into_response();
        return attach_session(response, session_id.as_deref());
    }
    if wants_sse {
        let channel = stream! {
            for response in responses.into_iter().flatten() {
                yield Ok::<Event, std::convert::Infallible>(
                    Event::default().event("message").data(response.to_string()),
                );
            }
        };
        let sse = Sse::new(channel)
            .keep_alive(KeepAlive::default())
            .into_response();
        return attach_session(sse, session_id.as_deref());
    }
    let response = if matches!(request, Value::Array(_)) {
        let results: Vec<_> = responses.into_iter().flatten().collect();
        json_response(StatusCode::OK, Value::Array(results))
    } else {
        match responses.into_iter().flatten().next() {
            Some(response) => json_response(StatusCode::OK, response),
            None => StatusCode::ACCEPTED.into_response(),
        }
    };
    attach_session(response, session_id.as_deref())
}

/// Normalize a request payload (a single message or a batch array) into its
/// JSON-RPC messages, so session and initialize detection can inspect all of them.
fn messages(request: Value) -> Box<dyn Iterator<Item = Value>> {
    match request {
        Value::Array(items) => Box::new(items.into_iter()),
        item => Box::new(std::iter::once(item)),
    }
}

fn is_initialize(message: Value) -> bool {
    message.get("method").and_then(Value::as_str) == Some("initialize")
}

/// Run every JSON-RPC message in a payload against the vault, preserving one
/// optional response per message (notifications produce `None`).
fn process_request(vault: &Vault, request: &Value, source_agent: &str) -> Vec<Option<Value>> {
    match request {
        Value::Array(items) => items
            .iter()
            .map(|item| handle_request(vault, item.clone(), source_agent))
            .collect(),
        value => vec![handle_request(vault, value.clone(), source_agent)],
    }
}

async fn oauth_metadata(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    if let Err(response) = validate_endpoint_headers(&state, &headers) {
        return *response;
    }
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("127.0.0.1");
    let issuer = format!("http://{host}");
    let resource = format!("{issuer}/mcp");
    let metadata = json!({
        "issuer": issuer,
        "resource": resource,
        "authorization_endpoint": format!("{issuer}/oauth/authorize"),
        "token_endpoint": format!("{issuer}/oauth/token"),
        "scopes_supported": ["mcp"],
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "service_documentation": OAUTH_TITLE,
    });
    json_response(StatusCode::OK, metadata)
}

fn attach_session(response: Response, session_id: Option<&str>) -> Response {
    let Some(session_id) = session_id else {
        return response;
    };
    let mut response = response;
    if let Ok(value) = HeaderValue::from_str(session_id) {
        response.headers_mut().insert(SESSION_HEADER, value);
    }
    response
}

fn validate_http_headers(state: &HttpState, headers: &HeaderMap) -> Result<(), Box<Response>> {
    validate_common_headers(state, headers)?;
    validate_post_headers(state, headers)
}

fn validate_common_headers(state: &HttpState, headers: &HeaderMap) -> Result<(), Box<Response>> {
    if let Some(origin) = headers.get(ORIGIN) {
        let allowed = origin
            .to_str()
            .ok()
            .is_some_and(|origin| state.allowed_origins.iter().any(|item| item == origin));
        if !allowed {
            return Err(Box::new(StatusCode::FORBIDDEN.into_response()));
        }
    }
    if let Some(token) = &state.bearer_token {
        let authorized = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == format!("Bearer {token}"));
        if !authorized {
            let mut response = StatusCode::UNAUTHORIZED.into_response();
            response.headers_mut().insert(
                WWW_AUTHENTICATE,
                HeaderValue::from_static("Bearer realm=\"relic\""),
            );
            return Err(Box::new(response));
        }
    }
    Ok(())
}

fn validate_post_headers(_state: &HttpState, headers: &HeaderMap) -> Result<(), Box<Response>> {
    let content_type_is_json = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(';').next() == Some("application/json"));
    if !content_type_is_json {
        return Err(Box::new(StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response()));
    }
    let accepts_required_types = headers
        .get(ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.contains("application/json") || value.contains("text/event-stream")
        });
    if !accepts_required_types {
        return Err(Box::new(StatusCode::NOT_ACCEPTABLE.into_response()));
    }
    if let Some(version) = headers.get("mcp-protocol-version")
        && version != HeaderValue::from_static(PROTOCOL_VERSION)
    {
        return Err(Box::new(StatusCode::BAD_REQUEST.into_response()));
    }
    Ok(())
}

/// Validate the origin and bearer token for a non-POST endpoint (SSE / SSE
/// discovery). These endpoints carry no JSON body, so content-type and
/// protocol-version checks do not apply.
fn validate_endpoint_headers(state: &HttpState, headers: &HeaderMap) -> Result<(), Box<Response>> {
    validate_common_headers(state, headers)
}

/// Accept a request carrying a known, unexpired session id; reject an unknown
/// or expired one with 404. A request with no session id is permitted so that
/// state-free clients keep working with this stateless-capable server.
fn validate_session(state: &HttpState, headers: &HeaderMap) -> Result<(), Box<Response>> {
    let Some(session_id) = headers
        .get(SESSION_HEADER)
        .and_then(|value| value.to_str().ok())
    else {
        return Ok(());
    };
    let valid = state
        .sessions
        .lock()
        .map(|mut sessions| match sessions.get_mut(session_id) {
            Some(last) if last.elapsed() < SESSION_IDLE_TTL => {
                *last = Instant::now();
                true
            }
            Some(_) => {
                sessions.remove(session_id);
                false
            }
            None => false,
        });
    if valid.unwrap_or(false) {
        Ok(())
    } else {
        Err(Box::new(StatusCode::NOT_FOUND.into_response()))
    }
}

fn json_response(status: StatusCode, value: Value) -> Response {
    let mut response = Response::new(Body::from(value.to_string()));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

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
            "capabilities": {
                "tools": { "listChanged": false },
                "oauth": { "supported": true }
            },
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
        "relic_capture" => {
            let mut input: crate::capture::CaptureInput = serde_json::from_value(arguments)?;
            if input.source_agent.is_empty() {
                input.source_agent = default_source_agent.into();
            }
            json!({"item": crate::capture::capture(vault, input)?})
        }
        "relic_list_captures" => {
            json!({"items": crate::capture::list(vault)?})
        }
        "relic_get_capture" => {
            let id = uuid::Uuid::parse_str(required_string(&arguments, "capture_id")?)?;
            let (source, item) = crate::capture::get(vault, id)?;
            json!({"source": source, "item": item})
        }
        "relic_process_captures" => {
            let limit = arguments
                .get("limit")
                .map(|v| v.as_u64().context("limit must be an integer"))
                .transpose()?
                .unwrap_or(100);
            let report = match optional_string(&arguments, "capture_id")? {
                Some(id) => crate::capture::work_one(vault, uuid::Uuid::parse_str(&id)?)?,
                None => crate::capture::work(vault, usize::try_from(limit)?)?,
            };
            json!({"report": report})
        }
        "relic_review_capture" => {
            let id = uuid::Uuid::parse_str(required_string(&arguments, "capture_id")?)?;
            let review =
                serde_json::from_value(arguments.get("review").context("missing review")?.clone())?;
            json!({"item": crate::capture::review(vault, id, review)?})
        }
        "relic_redistill_capture" => {
            let id = uuid::Uuid::parse_str(required_string(&arguments, "capture_id")?)?;
            json!({"item": crate::capture::redistill(vault, id)?})
        }
        "relic_retry_capture" => {
            let id = uuid::Uuid::parse_str(required_string(&arguments, "capture_id")?)?;
            json!({"item": crate::capture::retry(vault, id)?})
        }
        "relic_search" => {
            let query = required_string(&arguments, "query")?;
            let top_k = arguments
                .get("top_k")
                .and_then(Value::as_u64)
                .unwrap_or(10)
                .clamp(1, 100) as usize;
            // Keyword stays the default so an existing client's results do not
            // change underneath it; hybrid is the mode to ask for when the
            // caller wants recall across wording.
            let mode = match optional_string(&arguments, "mode")? {
                Some(value) => crate::search::Mode::parse(&value)?,
                None => crate::search::Mode::Keyword,
            };
            json!({ "mode": mode.as_str(), "results": vault.hybrid_search(query, top_k, mode)? })
        }
        "relic_find_similar" => {
            let top_k = arguments
                .get("top_k")
                .and_then(Value::as_u64)
                .unwrap_or(10)
                .clamp(1, 100) as usize;
            let minimum = arguments
                .get("min_similarity")
                .and_then(Value::as_f64)
                .unwrap_or(
                    vault
                        .config()
                        .map(|config| config.graph.semantic_min_similarity)
                        .unwrap_or(0.08),
                );
            match optional_string(&arguments, "entry_id")? {
                Some(entry_id) => {
                    let entry = vault.get(&entry_id)?;
                    json!({
                        "entry_id": entry.meta.id,
                        "title": entry.meta.title,
                        "results": vault.similar(&entry.meta.id, top_k, minimum)?,
                    })
                }
                None => {
                    let text = required_string(&arguments, "text")?;
                    json!({ "text": text, "results": vault.semantic_queries(text, top_k)? })
                }
            }
        }
        "relic_graph_neighbors" => {
            let entry_id = required_string(&arguments, "entry_id")?;
            let depth = arguments
                .get("depth")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .clamp(1, 4) as usize;
            let minimum = arguments
                .get("min_weight")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let kinds = crate::graph::parse_kinds(&string_array(&arguments, "kinds")?)?;
            let hits = vault
                .graph()?
                .neighborhood(entry_id, depth, minimum, kinds.as_deref())?;
            json!({ "entry_id": entry_id, "depth": depth, "neighbors": hits })
        }
        "relic_graph_path" => {
            let from = required_string(&arguments, "from_entry_id")?;
            let to = required_string(&arguments, "to_entry_id")?;
            let minimum = arguments
                .get("min_weight")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let path = vault.graph()?.path(from, to, minimum)?;
            json!({ "from": from, "to": to, "path": path })
        }
        "relic_graph_stats" => {
            let minimum = arguments
                .get("min_weight")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            json!({ "stats": vault.graph()?.stats(minimum) })
        }
        "relic_explain_relation" => {
            let from = required_string(&arguments, "from_entry_id")?;
            let to = required_string(&arguments, "to_entry_id")?;
            json!({ "from": from, "to": to, "relations": vault.graph()?.explain(from, to) })
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
            let average_effective_confidence = if entries.is_empty() {
                0.0
            } else {
                entries
                    .iter()
                    .map(|entry| entry.meta.effective_confidence())
                    .sum::<f64>()
                    / entries.len() as f64
            };
            json!({ "entries": entries.len(), "active": active, "fading": fading, "average_confidence": average_confidence, "average_effective_confidence": average_effective_confidence })
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
            "Search the local knowledge vault. Search before creating entries. mode=keyword is literal and precise; mode=semantic ranks by vector similarity and also finds memories phrased differently, including Chinese; mode=hybrid fuses both rankings.",
            json!({"type":"object","properties":{"query":{"type":"string"},"top_k":{"type":"integer","minimum":1,"maximum":100,"default":10},"mode":{"enum":["keyword","semantic","hybrid"],"default":"keyword"}},"required":["query"],"additionalProperties":false}),
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
        tool(
            "relic_capture",
            "Durably capture a structured experience for review. Reuse the same event_id, project, session_id and source_agent on retry. Submit no secrets or raw logs. This does not publish knowledge.",
            json!({"type":"object","properties":{
                "event_id":{"type":"string"},"project":{"type":"string"},"session_id":{"type":"string"},
                "source_agent":{"type":"string"},"title":{"type":"string"},"context":{"type":"string"},
                "action":{"type":"string"},"outcome":{"type":"string"},
                "evidence":{"type":"array","items":{"type":"string"}},"tags":{"type":"array","items":{"type":"string"}}
            },"required":["event_id","project","session_id","title","context","action","outcome"],"additionalProperties":false}),
            false,
            false,
            true,
        ),
        tool(
            "relic_list_captures",
            "List local capture states and candidate drafts; candidates are excluded from knowledge search.",
            json!({"type":"object","properties":{},"additionalProperties":false}),
            true,
            false,
            true,
        ),
        tool(
            "relic_get_capture",
            "Read a captured source event and its current review draft.",
            json!({"type":"object","properties":{"capture_id":{"type":"string","format":"uuid"}},"required":["capture_id"],"additionalProperties":false}),
            true,
            false,
            true,
        ),
        tool(
            "relic_process_captures",
            "Prepare drafts using the locally configured distiller (templates by default) and recover interrupted work. Evidence is not automatically verified.",
            json!({"type":"object","properties":{"limit":{"type":"integer","minimum":1,"maximum":1000,"default":100},"capture_id":{"type":"string","format":"uuid"}},"additionalProperties":false}),
            false,
            false,
            false,
        ),
        tool(
            "relic_review_capture",
            "Review a candidate after checking its evidence and related entries. Accept publishes a new Markdown entry; reject preserves the source. Supply final knowledge to replace the draft. Retry with the same review after an error.",
            json!({"type":"object","properties":{
                "capture_id":{"type":"string","format":"uuid"},
                "review":{"type":"object","properties":{
                    "decision":{"enum":["accept","reject"]},"reason":{"type":"string"},
                    "knowledge":{"type":"object","properties":{
                        "title":{"type":"string"},"content":{"type":"string"},"kind":{"enum":["knowledge","lesson","decision","pattern"]},
                        "confidence":{"type":"number","minimum":0,"maximum":1},"tags":{"type":"array","items":{"type":"string"}}
                    },"required":["title","content","kind","confidence","tags"],"additionalProperties":false}
                },"required":["decision","reason"],"additionalProperties":false}
            },"required":["capture_id","review"],"additionalProperties":false}),
            false,
            false,
            true,
        ),
        tool(
            "relic_retry_capture",
            "Requeue a failed capture after addressing the error; repeated retry before processing is safe.",
            json!({"type":"object","properties":{"capture_id":{"type":"string","format":"uuid"}},"required":["capture_id"],"additionalProperties":false}),
            false,
            false,
            true,
        ),
        tool(
            "relic_redistill_capture",
            "Requeue an unreviewed candidate using the current locally configured distiller. Run relic_process_captures afterwards. Accepted entries and recorded reviews cannot be reprocessed.",
            json!({"type":"object","properties":{"capture_id":{"type":"string","format":"uuid"}},"required":["capture_id"],"additionalProperties":false}),
            false,
            false,
            false,
        ),
        tool(
            "relic_find_similar",
            "Find memories closest to a memory or to free text in the vault's vector space. Returns cosine similarity and the vocabulary that carried it, so the relation can be judged rather than trusted. Use this to surface related knowledge that shares no keywords.",
            json!({"type":"object","properties":{"entry_id":{"type":"string"},"text":{"type":"string"},"top_k":{"type":"integer","minimum":1,"maximum":100,"default":10},"min_similarity":{"type":"number","minimum":0,"maximum":1}},"additionalProperties":false}),
            true,
            false,
            true,
        ),
        tool(
            "relic_graph_neighbors",
            "List memories connected to one memory by typed, evidence-carrying relations: explicit links, version history, shared subjects, vector proximity, contradictions, and corroborations. Filter by relation kind, hop depth, or minimum relation weight to see the backbone instead of the full network.",
            json!({"type":"object","properties":{"entry_id":{"type":"string"},"depth":{"type":"integer","minimum":1,"maximum":4,"default":1},"min_weight":{"type":"number","minimum":0,"maximum":1,"default":0},"kinds":{"type":"array","items":{"enum":["link","supersedes","tag","semantic","contradicts","corroborates"]}}},"required":["entry_id"],"additionalProperties":false}),
            true,
            false,
            true,
        ),
        tool(
            "relic_graph_path",
            "Find the strongest chain of relations between two memories. The route maximises its weakest hop, so the reported bottleneck is the confidence of the whole chain. Use this to explain how two pieces of knowledge are connected.",
            json!({"type":"object","properties":{"from_entry_id":{"type":"string"},"to_entry_id":{"type":"string"},"min_weight":{"type":"number","minimum":0,"maximum":1,"default":0}},"required":["from_entry_id","to_entry_id"],"additionalProperties":false}),
            true,
            false,
            true,
        ),
        tool(
            "relic_explain_relation",
            "Explain every relation recorded between two memories, with the kind, the derivation (explicit, statistical, or logical), the weight, and the concrete evidence. Use this before acting on a relation that was inferred rather than declared.",
            json!({"type":"object","properties":{"from_entry_id":{"type":"string"},"to_entry_id":{"type":"string"}},"required":["from_entry_id","to_entry_id"],"additionalProperties":false}),
            true,
            false,
            true,
        ),
        tool(
            "relic_graph_stats",
            "Summarise the relation network: counts by relation kind and derivation, connected components, hub memories, isolated memories, and any bounded derivation. Use this to judge whether the vault's knowledge is connected.",
            json!({"type":"object","properties":{"min_weight":{"type":"number","minimum":0,"maximum":1,"default":0}},"additionalProperties":false}),
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
