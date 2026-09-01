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
async fn declines_standalone_sse_streams() {
    let directory = tempdir().unwrap();
    let app = http_router(
        Vault::init(directory.path()).unwrap(),
        7337,
        "test-http",
        None,
        vec![],
    );
    let response = app
        .oneshot(Request::get("/mcp").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}
