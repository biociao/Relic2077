use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use relic2077::mcp::http_router;
use relic2077::vault::Vault;
use serde_json::{Value, json};
use tempfile::tempdir;
use tower::ServiceExt;

fn request(body: Value) -> Request<Body> {
    Request::post("/mcp")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn initializes_over_streamable_http() {
    let directory = tempdir().unwrap();
    let app = http_router(
        Vault::init(directory.path()).unwrap(),
        7337,
        "test-http",
        None,
        vec![],
    );
    let response = app
        .oneshot(request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/json");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["result"]["protocolVersion"], "2025-06-18");
}

#[tokio::test]
async fn accepts_notifications_without_a_response_body() {
    let directory = tempdir().unwrap();
    let app = http_router(
        Vault::init(directory.path()).unwrap(),
        7337,
        "test-http",
        None,
        vec![],
    );
    let response = app
        .oneshot(request(json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn rejects_untrusted_browser_origins() {
    let directory = tempdir().unwrap();
    let app = http_router(
        Vault::init(directory.path()).unwrap(),
        7337,
        "test-http",
        None,
        vec![],
    );
    let mut request = request(json!({"jsonrpc":"2.0","id":1,"method":"ping"}));
    request
        .headers_mut()
        .insert("origin", "https://evil.example".parse().unwrap());

    assert_eq!(
        app.oneshot(request).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn enforces_the_configured_bearer_token() {
    let directory = tempdir().unwrap();
    let app = http_router(
        Vault::init(directory.path()).unwrap(),
        7337,
        "test-http",
        Some("secret".into()),
        vec![],
    );
    let unauthorized = app
        .clone()
        .oneshot(request(json!({"jsonrpc":"2.0","id":1,"method":"ping"})))
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let mut authorized = request(json!({"jsonrpc":"2.0","id":2,"method":"ping"}));
    authorized
        .headers_mut()
        .insert("authorization", "Bearer secret".parse().unwrap());
    assert_eq!(
        app.oneshot(authorized).await.unwrap().status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn opens_an_sse_stream_on_get() {
    let directory = tempdir().unwrap();
    let app = http_router(
        Vault::init(directory.path()).unwrap(),
        7337,
        "test-http",
        None,
        vec![],
    );
    let response = app
        .oneshot(
            Request::get("/mcp")
                .header("accept", "text/event-stream")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
}

#[tokio::test]
async fn streams_responses_as_sse_events_when_requested() {
    let directory = tempdir().unwrap();
    let app = http_router(
        Vault::init(directory.path()).unwrap(),
        7337,
        "test-http",
        None,
        vec![],
    );
    let response = app
        .oneshot(
            Request::post("/mcp")
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .body(Body::from(
                    json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(body.contains("event: message"));
    assert!(body.contains("\"protocolVersion\":\"2025-06-18\""));
}

#[tokio::test]
async fn issues_and_requires_a_session_id_after_initialize() {
    let directory = tempdir().unwrap();
    let app = http_router(
        Vault::init(directory.path()).unwrap(),
        7337,
        "test-http",
        None,
        vec![],
    );
    let initialized = app
        .clone()
        .oneshot(request(json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}
        })))
        .await
        .unwrap();
    assert_eq!(initialized.status(), StatusCode::OK);
    let session_id = initialized.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(!session_id.is_empty());

    // A subsequent request carrying the session id is accepted.
    let mut with_session = request(json!({"jsonrpc":"2.0","id":2,"method":"ping"}));
    with_session
        .headers_mut()
        .insert("mcp-session-id", session_id.parse().unwrap());
    assert_eq!(
        app.clone().oneshot(with_session).await.unwrap().status(),
        StatusCode::OK
    );

    // An unknown session id is rejected with 404.
    let mut unknown = request(json!({"jsonrpc":"2.0","id":3,"method":"ping"}));
    unknown
        .headers_mut()
        .insert("mcp-session-id", "does-not-exist".parse().unwrap());
    assert_eq!(
        app.oneshot(unknown).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn answers_a_batch_as_a_json_array() {
    let directory = tempdir().unwrap();
    let app = http_router(
        Vault::init(directory.path()).unwrap(),
        7337,
        "test-http",
        None,
        vec![],
    );
    let response = app
        .oneshot(request(json!([
            {"jsonrpc":"2.0","id":1,"method":"ping"},
            {"jsonrpc":"2.0","id":2,"method":"ping"}
        ])))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/json");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body.is_array());
    assert_eq!(body.as_array().unwrap().len(), 2);
    assert_eq!(body[0]["result"], json!({}));
}

#[tokio::test]
async fn exposes_oauth_authorization_server_metadata() {
    let directory = tempdir().unwrap();
    let app = http_router(
        Vault::init(directory.path()).unwrap(),
        7337,
        "test-http",
        None,
        vec![],
    );
    let response = app
        .oneshot(
            Request::get("/.well-known/oauth-authorization-server")
                .header("host", "127.0.0.1:7337")
                .header("accept", "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["scopes_supported"][0], "mcp");
    assert_eq!(body["issuer"], "http://127.0.0.1:7337");
}
