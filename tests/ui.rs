use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use relic2077::ui::router;
use relic2077::vault::{EntryPatch, Vault};
use serde_json::{Value, json};
use std::fs;
use tempfile::tempdir;
use tower::ServiceExt;

fn request(method: Method, path: &str, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "localhost:7338")
        .header("origin", "http://localhost:7338");
    let body = if let Some(body) = body {
        builder = builder
            .header("content-type", "application/json")
            .header("x-relic-ui", "1");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    builder.body(body).unwrap()
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&body)
        .unwrap_or_else(|error| panic!("invalid JSON response ({status}): {error}: {body:?}"));
    (status, value)
}

fn query_value(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

#[tokio::test]
async fn embeds_the_interface_and_limits_browser_capabilities() {
    let directory = tempdir().unwrap();
    let app = router(Vault::init(directory.path()).unwrap(), 7338);
    for (path, content_type) in [
        ("/", "text/html"),
        ("/app.js", "javascript"),
        ("/brain.js", "javascript"),
        ("/stats.js", "javascript"),
        ("/styles.css", "text/css"),
    ] {
        let response = app
            .clone()
            .oneshot(request(Method::GET, path, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .contains(content_type),
            "{path} must have an executable/renderable content type"
        );
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        let csp = response.headers()["content-security-policy"]
            .to_str()
            .unwrap();
        assert!(csp.contains("default-src 'none'") || csp.contains("default-src 'self'"));
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(csp.contains("default-src 'none'") || csp.contains("object-src 'none'"));
        assert!(!csp.contains("'unsafe-eval'"));
        assert!(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .len()
                > 100
        );
    }
}

#[tokio::test]
async fn rejects_rebinding_cross_origin_and_browser_form_mutations() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let app = router(vault.clone(), 7338);
    for host in [None, Some("evil.example:7338"), Some("localhost:7337")] {
        let mut request = request(Method::GET, "/api/overview", None);
        request.headers_mut().remove("origin");
        request.headers_mut().remove("host");
        if let Some(host) = host {
            request.headers_mut().insert("host", host.parse().unwrap());
        }
        let (status, body) = send(&app, request).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "host: {host:?}");
        assert!(body["error"].is_string());
    }
    for origin in [
        "https://evil.example",
        "http://localhost:7337",
        "http://127.0.0.1:7338",
        "null",
    ] {
        let mut request = request(Method::GET, "/api/overview", None);
        request
            .headers_mut()
            .insert("origin", origin.parse().unwrap());
        assert_eq!(send(&app, request).await.0, StatusCode::FORBIDDEN);
    }
    let payload = json!({"title":"Blocked", "content":"Do not persist", "kind":"knowledge"});
    let mut missing_header = request(Method::POST, "/api/entries", Some(payload.clone()));
    missing_header.headers_mut().remove("x-relic-ui");
    assert_eq!(send(&app, missing_header).await.0, StatusCode::FORBIDDEN);
    let mut cross_site = request(Method::POST, "/api/entries", Some(payload.clone()));
    cross_site
        .headers_mut()
        .insert("sec-fetch-site", "cross-site".parse().unwrap());
    assert_eq!(send(&app, cross_site).await.0, StatusCode::FORBIDDEN);
    let mut non_json = request(Method::POST, "/api/entries", Some(payload));
    non_json
        .headers_mut()
        .insert("content-type", "text/plain".parse().unwrap());
    assert_eq!(
        send(&app, non_json).await.0,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    assert!(vault.entries().unwrap().is_empty());

    // A local CLI client need not fabricate browser Origin headers.
    for host in ["localhost:7338", "127.0.0.1:7338", "[::1]:7338"] {
        let mut local = request(Method::GET, "/api/overview", None);
        local.headers_mut().remove("origin");
        local.headers_mut().insert("host", host.parse().unwrap());
        assert_eq!(send(&app, local).await.0, StatusCode::OK);
    }
}

#[tokio::test]
async fn brain_graph_clusters_memories_into_topics_with_satellites() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    // Three memories that announce the same subject, which is the strongest
    // topic signal a real vault offers.
    for (title, tags, agent) in [
        (
            "GI02_JITC_rebuttal — Treg evidence audit, statistics, and claim calibration · Reusable knowledge · First",
            vec!["clinical".to_string()],
            "codex",
        ),
        (
            "GI02_JITC_rebuttal — Treg evidence audit, statistics, and claim calibration · Failures and how to do differently · Second",
            vec!["clinical".to_string(), "statistics".to_string()],
            "dsh",
        ),
        (
            "GI02_JITC_rebuttal — Treg evidence audit, statistics, and claim calibration · User preferences · Third",
            vec!["clinical".to_string()],
            "codex",
        ),
    ] {
        vault
            .create(
                title,
                "Body text about the audit",
                "knowledge",
                tags,
                0.6,
                agent,
            )
            .unwrap();
    }
    // Two more memories that announce no subject and share one non-marker tag,
    // so they cluster by tag instead.
    vault
        .create(
            "First vault note",
            "Body",
            "knowledge",
            vec!["relic2077".into()],
            0.8,
            "dsh",
        )
        .unwrap();
    vault
        .create(
            "Second vault note",
            "Body",
            "knowledge",
            vec!["relic2077".into()],
            0.7,
            "dsh",
        )
        .unwrap();

    let app = router(vault, 7338);
    let (status, brain) = send(&app, request(Method::GET, "/api/brain", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(brain["stats"]["total"], 5);
    assert_eq!(brain["stats"]["active"], 5);

    let topics = brain["topics"].as_array().unwrap();
    assert!(!topics.is_empty(), "expected at least one topic: {brain}");
    assert_eq!(brain["vault"]["name"], "Relic Vault");

    // The subject cluster is the primary node, and it carries evidence.
    let subject_topic = topics
        .iter()
        .find(|topic| {
            topic["label"]
                .as_str()
                .unwrap()
                .contains("GI02_JITC_rebuttal")
        })
        .unwrap_or_else(|| panic!("missing subject topic: {topics:?}"));
    assert_eq!(subject_topic["basis"], "subject");
    assert_eq!(subject_topic["count"], 3);
    assert!((subject_topic["share"].as_f64().unwrap() - 3.0 / 5.0).abs() < 1e-9);
    assert_eq!(subject_topic["kinds"]["knowledge"], 3);
    assert_eq!(subject_topic["agents"].as_array().unwrap().len(), 2);
    assert!(subject_topic["examples"].as_array().unwrap().len() == 3);
    let keywords = subject_topic["keywords"].as_array().unwrap();
    // The subject's own words must surface as keywords; which of them make the
    // top six depends on the corpus, so two stable ones are asserted.
    assert!(
        keywords
            .iter()
            .any(|keyword| keyword == "gi02_jitc_rebuttal")
            && keywords.iter().any(|keyword| keyword == "calibration"),
        "subject words become keywords: {keywords:?}"
    );
    assert!(
        subject_topic["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tag| tag == "clinical"),
        "a topic must name its own tags"
    );

    // Every edge is resolvable and weighted, and satellites carry a group.
    let edge_ids = brain["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| edge["to"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert!(edge_ids.iter().any(|id| id == "tag:clinical"));
    assert!(edge_ids.iter().any(|id| id.starts_with("keyword:")));
    assert!(edge_ids.iter().any(|id| id == "agent:codex"));
    assert!(
        brain["edges"]
            .as_array()
            .unwrap()
            .iter()
            .all(|edge| edge["weight"].as_u64().unwrap() >= 1)
    );
    for group in ["tags", "keywords", "agents"] {
        for node in brain[group].as_array().unwrap() {
            assert!(node["count"].as_u64().unwrap() >= 1, "{group}: {node}");
            assert!(!node["id"].as_str().unwrap().is_empty());
        }
    }
    assert!(brain["truncated"]["topics"].as_u64().is_some());
}

#[tokio::test]
async fn brain_graph_is_empty_and_bounded_without_memories() {
    let directory = tempdir().unwrap();
    let app = router(Vault::init(directory.path()).unwrap(), 7338);
    let (status, empty) = send(&app, request(Method::GET, "/api/brain", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(empty["topics"].as_array().unwrap().is_empty());
    assert!(empty["edges"].as_array().unwrap().is_empty());
    assert_eq!(empty["stats"]["total"], 0);
    assert_eq!(empty["stats"]["average_confidence"], 0.0);

    // An over-long label is dropped rather than shipped to the browser, and the
    // tag satellite budget stays inside its documented bound.
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let long_tag = "t".repeat(80);
    for index in 0..30 {
        vault
            .create(
                &format!("Note {index}"),
                "Body",
                "knowledge",
                vec![long_tag.clone(), format!("tag-{index}")],
                0.7,
                "codex",
            )
            .unwrap();
    }
    let app = router(vault, 7338);
    let (status, brain) = send(&app, request(Method::GET, "/api/brain", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        brain["tags"]
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node["id"].as_str().unwrap() != long_tag),
        "an 80-byte label must be dropped"
    );
    assert!(brain["tags"].as_array().unwrap().len() <= 26);
    assert!(brain["topics"].as_array().unwrap().len() <= 7);
    assert!(brain["truncated"]["tags"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn overview_reports_persisted_statuses_and_effective_confidence() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    for (title, status, confidence) in [
        ("Review me", "active", 0.2),
        ("Aging", "fading", 0.5),
        ("Replaced", "superseded", 0.7),
        ("History", "archived", 0.9),
    ] {
        let mut entry = vault
            .create(
                title,
                "A real memory",
                "knowledge",
                vec![],
                confidence,
                "codex",
            )
            .unwrap();
        if title == "Review me" {
            entry.meta.expires = Some(Utc::now() - Duration::days(1));
            fs::write(&entry.path, entry.render().unwrap()).unwrap();
        }
        vault
            .update(
                &entry.meta.id,
                EntryPatch {
                    status: Some(status.into()),
                    ..EntryPatch::default()
                },
            )
            .unwrap();
    }
    let app = router(vault, 7338);
    let (status, overview) = send(&app, request(Method::GET, "/api/overview", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(overview["vault"]["name"], "Relic Vault");
    assert_eq!(overview["stats"]["total"], 4);
    for status in ["active", "fading", "superseded", "archived"] {
        assert_eq!(overview["stats"][status], 1);
    }
    assert!(overview["stats"]["needs_review"].as_u64().unwrap() >= 1);
    let average = overview["stats"]["average_confidence"].as_f64().unwrap();
    assert!((average - 0.525).abs() < 0.0001);
    assert_eq!(overview["by_kind"]["knowledge"], 4);
    assert_eq!(overview["by_agent"]["codex"], 4);
    assert_eq!(overview["recent"].as_array().unwrap().len(), 4);
    let expired = overview["recent"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["meta"]["title"] == "Review me")
        .unwrap();
    assert_eq!(expired["effective_confidence"], 0.0);
    assert!(!overview["health"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn searches_unicode_and_combines_filters_before_pagination() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let mut expected_ids = Vec::new();
    for (title, kind, tag, confidence, agent, archived) in [
        ("长期记忆一", "knowledge", "系统", 0.9, "codex", false),
        ("长期记忆二", "knowledge", "系统", 0.85, "codex", false),
        ("长期记忆低分", "knowledge", "系统", 0.2, "codex", false),
        ("长期记忆决策", "decision", "系统", 0.9, "codex", false),
        ("长期记忆归档", "knowledge", "系统", 0.9, "codex", true),
        ("长期记忆来源", "knowledge", "系统", 0.9, "claude", false),
        ("长期记忆标签", "knowledge", "其他", 0.9, "codex", false),
        ("无关内容", "knowledge", "系统", 0.9, "codex", false),
    ] {
        let entry = vault
            .create(
                title,
                "UTF-8 内容可以搜索。",
                kind,
                vec![tag.into()],
                confidence,
                agent,
            )
            .unwrap();
        if expected_ids.len() < 2 {
            expected_ids.push(entry.meta.id.clone());
        }
        if archived {
            vault
                .update(
                    &entry.meta.id,
                    EntryPatch {
                        status: Some("archived".into()),
                        ..EntryPatch::default()
                    },
                )
                .unwrap();
        }
    }
    let app = router(vault, 7338);
    let filters = format!(
        "/api/entries?q={}&kind=knowledge&status=active&tag={}&source_agent=codex&min_confidence=0.8&limit=1",
        query_value("长期记忆"),
        query_value("系统")
    );
    let mut actual_ids = Vec::new();
    for offset in 0..3 {
        let (status, page) = send(
            &app,
            request(Method::GET, &format!("{filters}&offset={offset}"), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["total"], 2);
        let entries = page["entries"].as_array().unwrap();
        if offset < 2 {
            assert_eq!(entries.len(), 1);
            actual_ids.push(entries[0]["meta"]["id"].as_str().unwrap().to_owned());
            assert!(entries[0]["effective_confidence"].as_f64().unwrap() >= 0.8);
        } else {
            assert!(entries.is_empty());
        }
        assert!(page["tags"].as_array().unwrap().contains(&json!("系统")));
        assert!(page["agents"].as_array().unwrap().contains(&json!("codex")));
    }
    actual_ids.sort();
    expected_ids.sort();
    assert_eq!(actual_ids, expected_ids);

    // Search must cover bodies too, including non-ASCII text.
    let (status, body_matches) = send(
        &app,
        request(
            Method::GET,
            &format!("/api/entries?q={}", query_value("可以搜索")),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body_matches["total"], 8);
}

#[tokio::test]
async fn creates_and_edits_markdown_without_replacing_identity_or_history() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let old = vault
        .create(
            "Prior guidance",
            "Original",
            "knowledge",
            vec![],
            0.5,
            "cli",
        )
        .unwrap();
    let app = router(vault.clone(), 7338);
    let (status, created) = send(
        &app,
        request(
            Method::POST,
            "/api/entries",
            Some(json!({
                "title":"跨平台记忆", "content":"初始内容", "kind":"decision",
                "tags":["ui"], "confidence":0.8, "source_agent":"codex"
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = created["entry"]["meta"]["id"].as_str().unwrap();
    let original = vault.get(id).unwrap();
    assert!(original.path.is_file());
    assert!(original.body.contains("初始内容"));
    vault.supersede(&old.meta.id, id).unwrap();
    let path = format!("/api/entries/{id}");
    let (status, loaded) = send(&app, request(Method::GET, &path, None)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, changed) = send(
        &app,
        request(
            Method::PATCH,
            &path,
            Some(json!({
                "title":"新的标题", "content":"# 更新\n\n保留 Markdown 内容。",
                "status":"archived", "confidence":0.96, "tags":["ui", "verified"],
                "expected_updated":loaded["entry"]["meta"]["updated"]
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let persisted = vault.get(id).unwrap();
    assert_eq!(persisted.path, original.path);
    assert_eq!(persisted.meta.id, original.meta.id);
    assert_eq!(persisted.meta.created, original.meta.created);
    assert_eq!(persisted.meta.source_agents, original.meta.source_agents);
    assert_eq!(persisted.meta.supersedes, vec![old.meta.id.clone()]);
    assert!(persisted.meta.links.contains(&old.meta.id));
    assert_eq!(persisted.meta.title, "新的标题");
    assert_eq!(persisted.body, "# 更新\n\n保留 Markdown 内容。");
    assert_eq!(persisted.meta.status, "archived");
    assert_eq!(persisted.meta.confidence, 0.96);
    assert_eq!(persisted.meta.tags, vec!["ui", "verified"]);
    assert_eq!(vault.get(&old.meta.id).unwrap().meta.status, "superseded");
    assert_eq!(changed["entry"]["meta"]["id"], id);

    let (status, conflict) = send(
        &app,
        request(
            Method::PATCH,
            &path,
            Some(json!({
                "title":"Stale overwrite",
                "expected_updated":loaded["entry"]["meta"]["updated"]
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(conflict["error"].is_string());
    assert_eq!(vault.get(id).unwrap().meta.title, "新的标题");
}

#[tokio::test]
async fn rejects_invalid_entry_edits_without_touching_the_file() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let entry = vault
        .create("Keep me", "Unchanged", "knowledge", vec![], 0.8, "codex")
        .unwrap();
    let before = fs::read_to_string(&entry.path).unwrap();
    let app = router(vault, 7338);
    for patch in [
        json!({"title":"   "}),
        json!({"confidence":1.1}),
        json!({"status":"deleted"}),
    ] {
        let mut patch = patch;
        patch["expected_updated"] = json!(entry.meta.updated);
        let (status, error) = send(
            &app,
            request(
                Method::PATCH,
                &format!("/api/entries/{}", entry.meta.id),
                Some(patch),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        assert!(error["error"].is_string());
        assert_eq!(fs::read_to_string(&entry.path).unwrap(), before);
    }
    let (status, error) = send(
        &app,
        request(Method::GET, "/api/entries/does-not-exist", None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(error["error"].is_string());
}

#[tokio::test]
async fn saves_raw_config_losslessly_and_rejects_invalid_or_stale_changes() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let config_path = directory.path().join(".relic/config.yaml");
    let original = fs::read_to_string(&config_path).unwrap();
    let original =
        format!("# Preserve user comments\n{original}future_extension:\n  enabled: true\n");
    fs::write(&config_path, &original).unwrap();
    let app = router(vault.clone(), 7338);
    let (status, loaded) = send(&app, request(Method::GET, "/api/config", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(loaded["raw"], original);
    assert!(loaded["error"].is_null());
    let updated = original.replace("name: Relic Vault", "name: 团队记忆");
    let (status, saved) = send(
        &app,
        request(
            Method::PUT,
            "/api/config",
            Some(json!({"raw":updated, "expected_raw":original})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(fs::read_to_string(&config_path).unwrap(), updated);
    assert_eq!(saved["raw"], updated);
    assert_eq!(vault.config().unwrap().vault.name, "团队记忆");
    for invalid in [
        "version: [broken",
        "version: 2\n",
        "version: 1\nvault:\n  default_confidence: 1.5\n",
    ] {
        let (status, error) = send(
            &app,
            request(
                Method::PUT,
                "/api/config",
                Some(json!({"raw":invalid, "expected_raw":updated})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        assert!(error["error"].is_string());
        assert_eq!(fs::read_to_string(&config_path).unwrap(), updated);
    }
    let (status, conflict) = send(
        &app,
        request(
            Method::PUT,
            "/api/config",
            Some(json!({"raw":original, "expected_raw":original})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{conflict}");
    assert_eq!(fs::read_to_string(&config_path).unwrap(), updated);
}

#[tokio::test]
async fn malformed_vault_data_remains_diagnosable_and_config_can_be_repaired() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let config_path = directory.path().join(".relic/config.yaml");
    let valid_config = fs::read_to_string(&config_path).unwrap();
    let app = router(vault, 7338);
    fs::write(&config_path, "version: 2\n").unwrap();
    fs::write(
        directory.path().join("entries/inbox/broken.md"),
        "broken front matter",
    )
    .unwrap();
    let (status, overview) = send(&app, request(Method::GET, "/api/overview", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(overview["stats"].is_null());
    let health = overview["health"].as_array().unwrap();
    assert!(health.iter().any(|item| item["status"] == "error"));
    assert!(
        health
            .iter()
            .any(|item| item["message"].as_str().unwrap_or("").contains("broken.md"))
    );
    let (status, config) = send(&app, request(Method::GET, "/api/config", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(config["raw"], "version: 2\n");
    assert!(config["config"].is_null());
    assert!(config["error"].is_string());
    let (status, repaired) = send(
        &app,
        request(
            Method::PUT,
            "/api/config",
            Some(json!({"raw":valid_config, "expected_raw":"version: 2\n"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{repaired}");
    assert!(repaired["error"].is_null());
    assert_eq!(fs::read_to_string(&config_path).unwrap(), valid_config);
}

#[tokio::test]
async fn reindex_rebuilds_the_database_from_markdown_on_disk() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let mut entry = vault
        .create(
            "Externally edited",
            "Before",
            "knowledge",
            vec![],
            0.8,
            "cli",
        )
        .unwrap();
    entry.body = "An external editor wrote the unique term refraction.".into();
    fs::write(&entry.path, entry.render().unwrap()).unwrap();
    let app = router(vault, 7338);
    let (status, result) = send(&app, request(Method::POST, "/api/reindex", Some(json!({})))).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["count"], 1);
    let index =
        relic2077::index::Index::open(&directory.path().join(".relic/index.sqlite")).unwrap();
    let hits = index.search("refraction", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, entry.meta.id);
}

#[tokio::test]
async fn detects_raw_editor_changes_without_requiring_updated_metadata() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let entry = vault
        .create("Manual edits", "Before", "knowledge", vec![], 0.8, "cli")
        .unwrap();
    let app = router(vault.clone(), 7338);
    let url = format!("/api/entries/{}", entry.meta.id);
    let (status, loaded) = send(&app, request(Method::GET, &url, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(loaded["entry"]["revision"].is_string());
    let raw = fs::read_to_string(&entry.path).unwrap();
    let edited = raw.replace("Before", "An external edit without changing updated");
    fs::write(&entry.path, &edited).unwrap();
    let (status, overview) = send(&app, request(Method::GET, "/api/overview", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        overview["health"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["name"] == "index" && item["status"] == "warning")
    );
    let (status, rejected) = send(
        &app,
        request(
            Method::PATCH,
            &url,
            Some(json!({
                "content": "Stale browser text",
                "expected_updated": loaded["entry"]["meta"]["updated"],
                "expected_revision": loaded["entry"]["revision"],
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
    assert_eq!(fs::read_to_string(&entry.path).unwrap(), edited);

    let (_, refreshed) = send(&app, request(Method::GET, &url, None)).await;
    assert_eq!(
        refreshed["entry"]["meta"]["updated"],
        loaded["entry"]["meta"]["updated"]
    );
    assert_ne!(refreshed["entry"]["revision"], loaded["entry"]["revision"]);
    let (status, saved) = send(
        &app,
        request(
            Method::PATCH,
            &url,
            Some(json!({
                "content": "Merged browser and external changes",
                "expected_updated": refreshed["entry"]["meta"]["updated"],
                "expected_revision": refreshed["entry"]["revision"],
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        vault.get(&entry.meta.id).unwrap().body,
        "Merged browser and external changes"
    );
    let (_, overview) = send(&app, request(Method::GET, "/api/overview", None)).await;
    assert!(
        overview["health"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["name"] == "index" && item["status"] == "ok")
    );
}

#[tokio::test]
async fn automation_api_reports_queue_and_protects_mutations() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    relic2077::hooks::handle(&vault, "codex", json!({"hook_event_name":"Stop","session_id":"test","cwd":"/project","turn_id":"1","last_assistant_message":"A proposed lesson"})).unwrap();
    let app = router(vault.clone(), 7338);
    let (status, data) = send(&app, request(Method::GET, "/api/automation", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(data["queue"][0]["status"], "pending");
    let denied = Request::builder()
        .method("POST")
        .uri("/api/automation/retry")
        .header("host", "localhost:7338")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, denied).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        send(
            &app,
            request(Method::POST, "/api/captures/work", Some(json!({})))
        )
        .await
        .0,
        StatusCode::OK
    );
    let id = data["queue"][0]["id"].as_str().unwrap();
    let (status, review) = send(
        &app,
        request(
            Method::POST,
            &format!("/api/captures/{id}/review"),
            Some(json!({"decision":"reject","reason":"No durable evidence","knowledge":null})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(review["status"], "rejected");
    assert!(vault.entries().unwrap().is_empty());
}

#[tokio::test]
async fn history_requires_explicit_posts_and_never_publishes_without_review() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(&directory.path().join("vault")).unwrap();
    let root = directory.path().join("sessions");
    fs::create_dir(&root).unwrap();
    let lines = [
        json!({"type":"session_meta","payload":{"id":"ui-history","cwd":"/different/project"}}),
        json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"t"}}),
        json!({"type":"event_msg","payload":{"type":"user_message","message":"Fix"}}),
        json!({"type":"event_msg","payload":{"type":"task_complete","last_agent_message":"Fixed","turn_id":"t"}}),
    ];
    fs::write(
        root.join("s.jsonl"),
        lines.iter().map(|v| format!("{v}\n")).collect::<String>(),
    )
    .unwrap();
    let app = router(Vault::discover(&vault.root).unwrap(), 7338);
    for path in ["/api/overview", "/api/automation", "/api/history/defaults"] {
        assert_eq!(
            send(&app, request(Method::GET, path, None)).await.0,
            StatusCode::OK
        );
    }
    assert!(relic2077::capture::list(&vault).unwrap().is_empty());
    assert_eq!(
        send(&app, request(Method::GET, "/api/history/scan", None))
            .await
            .0,
        StatusCode::METHOD_NOT_ALLOWED
    );
    let body = json!({"host":"codex","root":root});
    let mut unsafe_request = request(Method::POST, "/api/history/scan", Some(body.clone()));
    unsafe_request.headers_mut().remove("x-relic-ui");
    assert_eq!(send(&app, unsafe_request).await.0, StatusCode::FORBIDDEN);
    let (status, data) = send(&app, request(Method::POST, "/api/history/scan", Some(body))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(relic2077::capture::list(&vault).unwrap().is_empty());
    let row = &data["sessions"][0];
    let body =
        json!({"host":"codex","root":root,"file":row["file"],"fingerprint":row["fingerprint"]});
    let (status, item) = send(
        &app,
        request(Method::POST, "/api/history/import", Some(body.clone())),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(item["status"], "pending");
    let (_, again) = send(
        &app,
        request(Method::POST, "/api/history/import", Some(body)),
    )
    .await;
    assert_eq!(item["id"], again["id"]);
    let path = format!("/api/history/{}/prepare", item["id"].as_str().unwrap());
    assert_eq!(
        send(&app, request(Method::POST, &path, Some(json!({}))))
            .await
            .0,
        StatusCode::OK
    );
    assert!(vault.entries().unwrap().is_empty());
    let (_, data) = send(&app, request(Method::GET, "/api/automation", None)).await;
    assert_eq!(data["queue"][0]["origin"], "manual_history");
    assert_eq!(data["queue"][0]["status"], "needs_review");
    assert_eq!(data["queue"][0]["session_id"], "ui-history");
}

#[tokio::test]
async fn memory_distribution_reports_kinds_lifecycle_tags_agents_and_confidence() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    // Two types, a shared tag, a repeated agent on one memory, and four
    // confidences that span the cut-offs the view reports.
    let mut latest = vault
        .create(
            "Design decision",
            "A decision body",
            "decision",
            vec!["architecture".into(), "ui".into()],
            0.62,
            "codex",
        )
        .unwrap();
    let reference = vault
        .create(
            "Reference",
            "A knowledge body",
            "knowledge",
            vec!["architecture".into()],
            0.55,
            "dsh",
        )
        .unwrap();
    vault
        .create(
            "Freshest",
            "Another knowledge body",
            "knowledge",
            vec!["architecture".into(), "ui".into()],
            0.9,
            "dsh",
        )
        .unwrap();
    vault
        .create(
            "Faint",
            "A lesson body",
            "lesson",
            vec!["architecture".into()],
            0.2,
            "codex",
        )
        .unwrap();
    // A second provenance entry for the same agent must not double count.
    latest.meta.source_agents = vec!["codex".into(), "codex".into()];
    latest.meta.last_verified = Utc::now();
    latest.meta.updated = Utc::now();
    vault
        .update(
            &latest.meta.id,
            EntryPatch {
                source_agents: Some(vec!["codex".into(), "codex".into()]),
                status: Some("fading".into()),
                ..EntryPatch::default()
            },
        )
        .unwrap();
    let reference_confidence = reference.meta.effective_confidence();

    let app = router(vault, 7338);
    let (status, stats) = send(&app, request(Method::GET, "/api/stats", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stats["scope"]["total"], 4);
    assert_eq!(stats["scope"]["filtered"], false);
    assert_eq!(stats["scope"]["vault_total"], 4);

    let distribution = stats["distribution"]["by_kind"].as_array().unwrap();
    assert_eq!(distribution.len(), 3, "{stats}");
    let kind = |name: &str| {
        distribution
            .iter()
            .find(|item| item["name"] == name)
            .unwrap_or_else(|| panic!("missing kind {name}"))
    };
    assert_eq!(kind("knowledge")["count"], 2);
    assert_eq!(kind("lesson")["count"], 1);
    assert_eq!(kind("decision")["count"], 1);

    let statuses = stats["distribution"]["by_status"].as_array().unwrap();
    assert_eq!(statuses.len(), 2, "fading and active are listed separately");
    assert_eq!(
        statuses
            .iter()
            .find(|item| item["name"] == "fading")
            .unwrap()["count"],
        1
    );

    let tags = stats["distribution"]["by_tag"].as_array().unwrap();
    let architecture = tags
        .iter()
        .find(|item| item["name"] == "architecture")
        .expect("the shared tag is one row");
    assert_eq!(architecture["count"], 4, "a tag counts memories, not links");
    assert_eq!(tags.len(), 2);
    let agents = stats["distribution"]["by_agent"].as_array().unwrap();
    let codex = agents
        .iter()
        .find(|item| item["name"] == "codex")
        .expect("codex is a source");
    assert_eq!(
        codex["count"], 2,
        "a repeated agent on one memory counts twice at most once"
    );
    assert_eq!(stats["labels"]["agents"], json!(["codex", "dsh"]));

    let confidence = &stats["confidence"];
    let bins = confidence["bins"].as_array().unwrap();
    assert_eq!(bins.len(), 10);
    assert_eq!(bins[0]["label"], "0.0–0.1");
    assert_eq!(bins[0]["count"], 0);
    assert_eq!(
        bins.iter()
            .map(|bin| bin["count"].as_u64().unwrap())
            .sum::<u64>(),
        4,
        "every memory falls in exactly one bin"
    );
    assert_eq!(
        bins.iter().find(|bin| bin["from"] == 0.2).unwrap()["count"],
        1
    );
    assert_eq!(confidence["thresholds"][0]["level"], 0.3);
    assert_eq!(confidence["thresholds"][0]["above"], 3);
    assert_eq!(confidence["thresholds"][0]["below"], 1);
    assert_eq!(confidence["thresholds"][2]["level"], 0.7);
    assert_eq!(confidence["thresholds"][2]["above"], 1);
    assert_eq!(confidence["thresholds"][2]["below"], 3);
    assert_eq!(confidence["thresholds"][3]["level"], 0.9);
    assert_eq!(confidence["thresholds"][3]["above"], 1);
    assert_eq!(confidence["thresholds"][3]["below"], 3);
    // The decayed confidence is what the view counts, not the stored value.
    let expected = (0.62_f64 + reference_confidence + 0.9 + 0.2) / 4.0;
    let mean = confidence["mean"].as_f64().unwrap();
    assert!((mean - expected).abs() < 1e-6, "mean {mean} != {expected}");
    assert!(confidence["median"].as_f64().unwrap() > 0.0);
    assert!(confidence["standard_deviation"].as_f64().unwrap() > 0.0);
    assert!((0.05..=0.14).contains(&confidence["bandwidth"].as_f64().unwrap()));
    assert!(
        bins.iter()
            .all(|bin| bin["density"].as_f64().unwrap().is_finite())
    );
    assert_eq!(stats["truncated"]["labels"], 0);
}

#[tokio::test]
async fn memory_distribution_follows_the_same_filters_as_the_memory_list() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    for (index, (kind, confidence, agent)) in [
        ("knowledge", 0.9, "codex"),
        ("knowledge", 0.8, "dsh"),
        ("lesson", 0.4, "codex"),
        ("decision", 0.2, "codex-memory"),
    ]
    .into_iter()
    .enumerate()
    {
        vault
            .create(
                &format!("Memory {index}"),
                "A body",
                kind,
                vec!["shared".into()],
                confidence,
                agent,
            )
            .unwrap();
    }
    let app = router(vault, 7338);

    // Every chart describes exactly the memories the list shows, so the two
    // endpoints must always agree on the count for one filter set.
    let cases = [
        "",
        "min_confidence=0.5",
        "kind=knowledge",
        "source_agent=codex",
        "tag=shared&status=active",
        "q=memory&kind=knowledge",
        "min_confidence=0.1&kind=lesson",
    ];
    for query in cases {
        let (_, stats) = send(
            &app,
            request(Method::GET, &format!("/api/stats?{query}"), None),
        )
        .await;
        let (_, entries) = send(
            &app,
            request(Method::GET, &format!("/api/entries?{query}"), None),
        )
        .await;
        assert_eq!(
            stats["scope"]["total"], entries["total"],
            "scope must match the list for '{query}'"
        );
        assert_eq!(stats["scope"]["vault_total"], 4, "{query}");
        let bins = stats["confidence"]["bins"].as_array().unwrap();
        assert_eq!(
            bins.iter()
                .map(|bin| bin["count"].as_u64().unwrap())
                .sum::<u64>() as usize,
            entries["total"].as_u64().unwrap() as usize,
            "bins must cover the whole scope for '{query}'"
        );
        for threshold in stats["confidence"]["thresholds"].as_array().unwrap() {
            assert_eq!(
                threshold["above"].as_u64().unwrap() + threshold["below"].as_u64().unwrap(),
                entries["total"].as_u64().unwrap(),
                "each cut-off must split the same scope for '{query}'"
            );
        }
    }

    let (_, filtered) = send(
        &app,
        request(Method::GET, "/api/stats?min_confidence=0.5", None),
    )
    .await;
    assert_eq!(filtered["scope"]["filtered"], true);
    assert_eq!(filtered["scope"]["total"], 2);
    assert_eq!(
        filtered["distribution"]["by_kind"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(filtered["confidence"]["thresholds"][0]["above"], 2);
    assert_eq!(filtered["confidence"]["thresholds"][0]["below"], 0);

    assert_eq!(
        send(
            &app,
            request(Method::GET, "/api/stats?min_confidence=1.5", None)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn memory_distribution_is_empty_and_finite_without_memories() {
    let directory = tempdir().unwrap();
    let app = router(Vault::init(directory.path()).unwrap(), 7338);
    let (status, stats) = send(&app, request(Method::GET, "/api/stats", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stats["scope"]["total"], 0);
    assert_eq!(stats["confidence"]["mean"], 0.0);
    assert_eq!(stats["confidence"]["median"], 0.0);
    assert!(
        stats["distribution"]["by_kind"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        stats["confidence"]["bins"]
            .as_array()
            .unwrap()
            .iter()
            .all(|bin| bin["count"] == 0 && bin["density"].as_f64().unwrap().is_finite())
    );
    assert_eq!(stats["confidence"]["thresholds"][3]["above"], 0);
}
