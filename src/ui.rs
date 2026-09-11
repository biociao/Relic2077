//! Embedded, loopback-only browser interface for a local vault.
//!
//! The UI shares the same Markdown files and index as the CLI and MCP server.
//! Its mutex serializes this server's writes; optimistic revisions also catch
//! changes made by another browser tab, an agent, or an editor.

use crate::config::Config;
use crate::entry::Entry;
use crate::vault::{EntryPatch, Vault};
use anyhow::{Context, Result, bail};
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct UiState {
    vault: Arc<Mutex<Vault>>,
    port: u16,
}

struct ApiError {
    status: StatusCode,
    message: String,
}

type ApiResult<T> = std::result::Result<T, ApiError>;

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, format!("{error:#}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

fn json_error(error: JsonRejection) -> ApiError {
    let status = match error.status() {
        StatusCode::PAYLOAD_TOO_LARGE => StatusCode::PAYLOAD_TOO_LARGE,
        StatusCode::UNSUPPORTED_MEDIA_TYPE => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        _ => StatusCode::BAD_REQUEST,
    };
    ApiError::new(status, error.body_text())
}

/// Build the UI router for the actual listening port. Hosts from other ports
/// are rejected, even when they resolve to the same loopback interface.
pub fn router(vault: Vault, port: u16) -> Router {
    let state = UiState {
        vault: Arc::new(Mutex::new(vault)),
        port,
    };
    Router::new()
        .route("/", get(index_html))
        .route("/index.html", get(index_html))
        .route("/app.js", get(app_js))
        .route("/brain.js", get(brain_js))
        .route("/stats.js", get(stats_js))
        .route("/graph.js", get(graph_js))
        .route("/styles.css", get(styles_css))
        .route("/brand.png", get(brand_image))
        .route("/api/overview", get(overview))
        .route("/api/brain", get(brain))
        .route("/api/graph", get(graph))
        .route("/api/stats", get(stats))
        .route("/api/entries", get(entries).post(create_entry))
        .route("/api/entries/{id}", get(get_entry).patch(update_entry))
        .route("/api/config", get(get_config).put(update_config))
        .route("/api/reindex", post(reindex))
        .route("/api/automation", get(automation_status))
        .route("/api/automation/retry", post(automation_retry))
        .route("/api/automation/setup", post(automation_setup))
        .route("/api/history/defaults", get(history_defaults))
        .route("/api/history/scan", post(history_scan))
        .route("/api/history/import", post(history_import))
        .route("/api/history/{id}/prepare", post(history_prepare))
        .route("/api/captures/work", post(captures_work))
        .route("/api/captures/{id}/review", post(captures_review))
        .route("/api/captures/{id}/retry", post(captures_retry))
        .fallback(|| async { ApiError::new(StatusCode::NOT_FOUND, "Route not found") })
        .method_not_allowed_fallback(|| async {
            ApiError::new(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed")
        })
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), protect))
        .with_state(state)
}

/// Serve the dashboard until the process exits. No frontend runtime, network
/// service, or platform-specific desktop dependency is required.
pub fn serve(vault: Vault, bind: SocketAddr) -> Result<()> {
    if !matches!(bind.ip(), IpAddr::V4(ip) if ip == Ipv4Addr::LOCALHOST)
        && !matches!(bind.ip(), IpAddr::V6(ip) if ip == Ipv6Addr::LOCALHOST)
    {
        bail!("Relic UI can only bind to a loopback address (127.0.0.1 or ::1)");
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(bind)
            .await
            .with_context(|| format!("could not start Relic UI on {bind}"))?;
        let address = listener.local_addr()?;
        println!("Relic UI: http://{address}");
        println!("Vault: {}", vault.root.display());
        println!("Open this address in your browser. Press Ctrl+C to stop.");
        axum::serve(listener, router(vault, address.port())).await?;
        Ok(())
    })
}

async fn protect(State(state): State<UiState>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    let host_allowed = host.is_some_and(|host| {
        ["localhost", "127.0.0.1", "[::1]"]
            .iter()
            .any(|allowed| host.eq_ignore_ascii_case(&format!("{allowed}:{}", state.port)))
    }) && headers.get_all(header::HOST).iter().count() == 1;
    let failure = if !host_allowed {
        Some("Host must identify this local Relic UI address")
    } else if headers.get_all(header::ORIGIN).iter().count() > 1
        || headers.get(header::ORIGIN).is_some_and(|origin| {
            origin.to_str().ok() != host.map(|host| format!("http://{host}")).as_deref()
        })
    {
        Some("Cross-origin requests are not allowed")
    } else if headers
        .get("sec-fetch-site")
        .is_some_and(|site| !matches!(site.to_str().ok(), Some("same-origin" | "none")))
    {
        Some("Cross-site requests are not allowed")
    } else if !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) && headers.get("x-relic-ui").and_then(|v| v.to_str().ok()) != Some("1")
    {
        Some("Mutations require the x-relic-ui: 1 header")
    } else {
        None
    };
    let mut response = match failure {
        Some(message) => ApiError::new(StatusCode::FORBIDDEN, message).into_response(),
        None => next.run(request).await,
    };
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
        ),
    );
    response
}

async fn index_html() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("../ui/index.html"),
    )
}

async fn app_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../ui/app.js"),
    )
}

/// The memory brain is a separate asset so the dashboard entry point stays
/// readable; it is still embedded in the binary like every other UI file.
async fn brain_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../ui/brain.js"),
    )
}

/// The distribution view of the memory library is its own asset for the same
/// reason: the dashboard entry point stays readable, and the binary still
/// carries every UI file it serves.
async fn stats_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../ui/stats.js"),
    )
}

/// The relation view is its own asset for the same reason as the distribution
/// view: the entry point stays readable, and the binary still carries every UI
/// file it serves.
async fn graph_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../ui/graph.js"),
    )
}

async fn styles_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../ui/styles.css"),
    )
}

async fn brand_image() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/png")],
        &include_bytes!("../assets/relic2077-agent-memory-core.png")[..],
    )
}

async fn with_vault<T, F>(state: UiState, operation: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(&Vault) -> ApiResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let vault = state.vault.lock().map_err(|_| {
            ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Vault lock unavailable")
        })?;
        operation(&vault)
    })
    .await
    .map_err(|error| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Vault task failed: {error}"),
        )
    })?
}

fn revision(raw: &str) -> String {
    // This is a change token, not an authentication credential. It covers the
    // exact bytes so even a manual edit that leaves `updated` intact conflicts.
    let mut hasher = DefaultHasher::new();
    raw.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn entry_json(vault: &Vault, entry: &Entry) -> Result<Value> {
    let raw = fs::read_to_string(&entry.path)?;
    // Pair the revision with the same snapshot as the returned body/metadata.
    let entry = Entry::parse(&entry.path, &raw)?;
    Ok(json!({
        "meta": entry.meta,
        "body": entry.body,
        "path": entry.path.strip_prefix(&vault.root).unwrap_or(&entry.path).to_string_lossy().replace('\\', "/"),
        "effective_confidence": entry.meta.effective_confidence(),
        "revision": revision(&raw),
    }))
}

fn read_entries(vault: &Vault) -> Result<Vec<Entry>> {
    // Vault::entries remains the source of truth. Additionally surface invalid
    // non-finite decay values, traversal failures, and duplicate IDs that make
    // UI edits ambiguous. Vault::entries skips traversal errors for CLI use.
    for folder in ["entries", "patterns", "decisions", "reflections"] {
        let root = vault.root.join(folder);
        if root.exists() {
            for item in walkdir::WalkDir::new(root) {
                item.context("could not traverse the memory directory")?;
            }
        }
    }
    let entries = vault.entries()?;
    let mut ids = BTreeSet::new();
    for entry in &entries {
        if !entry.meta.decay_rate.is_finite() {
            bail!(
                "invalid entry {}: decay_rate must be finite",
                entry.path.display()
            );
        }
        if !ids.insert(&entry.meta.id) {
            bail!("duplicate entry id '{}'", entry.meta.id);
        }
    }
    Ok(entries)
}

fn find_entry(vault: &Vault, id: &str) -> ApiResult<Entry> {
    read_entries(vault)?
        .into_iter()
        .find(|entry| entry.meta.id == id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("Entry '{id}' not found")))
}

fn health(name: &str, status: &str, message: impl Into<String>) -> Value {
    json!({"name": name, "status": status, "message": message.into()})
}

async fn overview(State(state): State<UiState>) -> ApiResult<Json<Value>> {
    with_vault(state, |vault| {
        let mut checks = Vec::new();
        let config = match vault.config() {
            Ok(config) => {
                checks.push(health("config", "ok", "Vault configuration is valid"));
                Some(config)
            }
            Err(error) => {
                checks.push(health("config", "error", format!("{error:#}")));
                None
            }
        };
        let entries = match read_entries(vault) {
            Ok(entries) => {
                checks.push(health("entries", "ok", format!("{} readable memory entries", entries.len())));
                Some(entries)
            }
            Err(error) => {
                checks.push(health("entries", "error", format!("{error:#}")));
                None
            }
        };
        checks.push(index_health(vault, entries.as_deref()));
        checks.push(if vault.root.ancestors().any(|root| root.join(".git").exists()) {
            health("git", "ok", "Git metadata is present; remote synchronization is managed by the CLI")
        } else {
            health("git", "warning", "This vault is not a Git repository; run git init to enable version history")
        });
        let mut by_kind = BTreeMap::<String, usize>::new();
        let mut by_agent = BTreeMap::<String, usize>::new();
        let mut recent = Vec::new();
        let mut activity = Vec::new();
        let stats = if let Some(entries) = &entries {
            let today = Utc::now().date_naive();
            for days_ago in (0..14).rev() {
                let day = today - Duration::days(days_ago);
                activity.push(json!({
                    "date": day.to_string(),
                    "count": entries.iter().filter(|e| e.meta.updated.date_naive() == day).count(),
                }));
            }
            let now = Utc::now();
            let threshold = config.as_ref().map(|c| c.evolution.fading_threshold).unwrap_or(0.3);
            for entry in entries {
                *by_kind.entry(entry.meta.kind.clone()).or_default() += 1;
                for agent in entry.meta.source_agents.iter().collect::<BTreeSet<_>>() {
                    *by_agent.entry(agent.clone()).or_default() += 1;
                }
            }
            recent = entries.iter().take(6).map(|entry| entry_json(vault, entry)).collect::<Result<Vec<_>>>()?;
            json!({
                "total": entries.len(),
                "active": entries.iter().filter(|e| e.meta.status == "active").count(),
                "fading": entries.iter().filter(|e| e.meta.status == "fading").count(),
                "superseded": entries.iter().filter(|e| e.meta.status == "superseded").count(),
                "archived": entries.iter().filter(|e| e.meta.status == "archived").count(),
                "average_confidence": if entries.is_empty() { 0.0 } else {
                    entries.iter().map(|e| e.meta.effective_confidence_at(now)).sum::<f64>() / entries.len() as f64
                },
                "needs_review": entries.iter().filter(|e|
                    matches!(e.meta.status.as_str(), "active" | "fading") &&
                    (e.meta.status == "fading" || e.meta.effective_confidence_at(now) < threshold)
                ).count(),
            })
        } else {
            Value::Null
        };
        Ok(Json(json!({
            "vault": {
                "name": config.as_ref().map(|c| c.vault.name.as_str()).unwrap_or("Relic Vault"),
                "path": vault.root,
                "version": config.as_ref().map(|c| c.version),
            },
            "stats": stats,
            "by_kind": by_kind,
            "by_agent": by_agent,
            "activity": activity,
            "recent": recent,
            "health": checks,
        })))
    })
    .await
}

/// One memory topic: a subject cluster the banner draws as a primary node.
///
/// A topic is derived, never stored: it groups memories that already share a
/// subject, tag, or distinctive keyword. `basis` records which signal produced
/// the cluster so the UI can say why these memories belong together.
struct BrainTopic {
    id: String,
    label: String,
    basis: &'static str,
    entries: Vec<usize>,
    tags: BTreeMap<String, usize>,
    keywords: BTreeMap<String, usize>,
    kinds: BTreeMap<String, usize>,
    statuses: BTreeMap<String, usize>,
}

/// Aggregate one tag, keyword, or source agent across the vault.
///
/// `kinds` is a weight distribution over memory types so the browser can show
/// what a node is made of, not only how large it is. `count` counts entries, so
/// a memory carrying two tags contributes once to each tag node.
struct BrainNode {
    id: String,
    label: String,
    count: usize,
    kinds: BTreeMap<String, usize>,
    example_id: String,
    example_title: String,
    updated: String,
}

fn collect_brain_node(
    nodes: &mut BTreeMap<String, BrainNode>,
    id: &str,
    kind: &str,
    entry: &Entry,
) {
    let node = nodes.entry(id.to_owned()).or_insert_with(|| BrainNode {
        id: id.to_owned(),
        label: id.to_owned(),
        count: 0,
        kinds: BTreeMap::new(),
        example_id: entry.meta.id.clone(),
        example_title: entry.meta.title.clone(),
        updated: entry.meta.updated.to_rfc3339(),
    });
    node.count += 1;
    *node.kinds.entry(kind.to_owned()).or_default() += 1;
}

fn brain_node_json(node: &BrainNode) -> Value {
    json!({
        "id": node.id,
        "label": node.label,
        "count": node.count,
        "kinds": node.kinds,
        "example": {"id": node.example_id, "title": node.example_title},
        "updated": node.updated,
    })
}

/// Words that carry no topic information in this vault's memory titles.
const BRAIN_STOPWORDS: &[&str] = &[
    "and",
    "the",
    "for",
    "with",
    "without",
    "from",
    "into",
    "onto",
    "over",
    "under",
    "then",
    "than",
    "that",
    "this",
    "these",
    "those",
    "its",
    "their",
    "his",
    "her",
    "our",
    "your",
    "not",
    "but",
    "are",
    "is",
    "was",
    "were",
    "be",
    "been",
    "being",
    "has",
    "have",
    "had",
    "does",
    "did",
    "do",
    "can",
    "could",
    "should",
    "would",
    "will",
    "would",
    "may",
    "might",
    "must",
    "use",
    "used",
    "using",
    "make",
    "made",
    "get",
    "got",
    "one",
    "two",
    "more",
    "most",
    "less",
    "few",
    "many",
    "all",
    "any",
    "some",
    "each",
    "every",
    "only",
    "also",
    "just",
    "when",
    "where",
    "which",
    "while",
    "what",
    "why",
    "how",
    "who",
    "whom",
    "before",
    "after",
    "during",
    "between",
    "per",
    "via",
    "vs",
    "versus",
    "about",
    "again",
    "against",
    "because",
    "so",
    "such",
    "no",
    "nor",
    "own",
    "same",
    "other",
    "another",
    "new",
    "old",
    "first",
    "second",
    "next",
    "last",
    "still",
    "already",
    "instead",
    "however",
    "therefore",
    "here",
    "there",
    "now",
    "ever",
    "never",
    "always",
    "often",
    "sometimes",
    "and/or",
    "etc",
    "status",
    "note",
    "notes",
    "result",
    "results",
    "case",
    "cases",
    "item",
    "items",
    "value",
    "values",
    "true",
    "false",
    "null",
    "none",
    "yes",
    "via",
    "na",
    "todo",
    "tbd",
    "unknown",
    "misc",
    "other",
    "others",
    "failures",
    "failure",
    "knowledge",
    "reusable",
    "preferences",
    "preference",
    "task",
    "tasks",
    "lesson",
    "lessons",
    "decision",
    "decisions",
    "pattern",
    "patterns",
    "reflection",
    "reflections",
    "entry",
    "entries",
    "meta",
    "title",
    "body",
    "tags",
    "tag",
    "http",
    "https",
    "www",
    "com",
    "active",
    "archived",
    "fading",
    "superseded",
    "verified",
    "unverified",
    "confidence",
    "effective",
    "threshold",
    "default",
    "defaults",
    "summary",
    "overview",
    "current",
    "recent",
];

/// A single token of a memory label: lowercase ASCII words and CJK bigrams.
///
/// CJK has no word separators, so overlapping bigrams stand in for words; the
/// corpus-wide filter in [`brain_keywords`] drops the meaningless ones.
fn brain_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut han: Vec<char> = Vec::new();
    let flush_word = |word: &mut String, tokens: &mut Vec<String>| {
        let lower = word.to_lowercase();
        let keep = |value: &str| {
            value.chars().count() >= 3
                && !BRAIN_STOPWORDS.contains(&value)
                && !value.chars().all(|c| c.is_numeric())
        };
        if keep(&lower) {
            tokens.push(lower.clone());
        }
        // Project names such as `publication-figure` are one topic word, so the
        // compound is kept whole as well as split into its parts.
        if lower.contains(['-', '_']) && lower.chars().count() >= 5 {
            for part in lower.split(['-', '_']) {
                if keep(part) && part != lower {
                    tokens.push(part.to_owned());
                }
            }
        }
        word.clear();
    };
    let flush_han = |han: &mut Vec<char>, tokens: &mut Vec<String>| {
        if han.len() >= 2 {
            if han.len() == 2 {
                tokens.push(han.iter().collect());
            } else {
                for pair in han.windows(2) {
                    tokens.push(pair.iter().collect());
                }
            }
        }
        han.clear();
    };
    for character in text.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '+' | '#' | '-') {
            flush_han(&mut han, &mut tokens);
            word.push(character);
        } else if character.is_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(&character) {
            flush_word(&mut word, &mut tokens);
            han.push(character);
        } else {
            flush_word(&mut word, &mut tokens);
            flush_han(&mut han, &mut tokens);
        }
    }
    flush_word(&mut word, &mut tokens);
    flush_han(&mut han, &mut tokens);
    tokens
}

/// Corpus document frequencies for keyword scoring.
///
/// Tokens come from titles only. Memory bodies are long prose (and often
/// Chinese), where overlapping bigrams explode into hundreds of thousands of
/// noise tokens; titles are short, dense, and already carry the subject.
fn token_frequencies<'a>(entries: impl Iterator<Item = &'a Entry>) -> BTreeMap<String, usize> {
    let mut frequencies = BTreeMap::new();
    for entry in entries {
        for token in brain_tokens(&entry.meta.title)
            .into_iter()
            .collect::<BTreeSet<_>>()
        {
            *frequencies.entry(token).or_default() += 1;
        }
    }
    frequencies
}

/// Distinctive keywords of a cluster: frequent inside it, rare outside.
///
/// Scoring by concentration (not raw counts) is what makes `pumch-isowast` show
/// up on its own topic instead of every large topic reporting "memory" and
/// "verified".
fn brain_keywords(
    entries: &[&Entry],
    total: usize,
    document_frequency: &BTreeMap<String, usize>,
    limit: usize,
) -> BTreeMap<String, usize> {
    let mut local = BTreeMap::<String, usize>::new();
    for entry in entries {
        for token in brain_tokens(&entry.meta.title)
            .into_iter()
            .collect::<BTreeSet<_>>()
        {
            *local.entry(token).or_default() += 1;
        }
    }
    let mut scored = local
        .into_iter()
        .filter_map(|(token, count)| {
            let frequency = *document_frequency.get(&token).unwrap_or(&count);
            // Too rare to describe anything, or so common it describes everything.
            if count < 2 || frequency * 3 > total.max(1) * 2 || token.chars().count() < 3 {
                return None;
            }
            let concentration = count as f64 / frequency as f64;
            Some((token, count, concentration * (count as f64).sqrt()))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|a, b| {
        b.2.total_cmp(&a.2)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.0.cmp(&b.0))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(token, count, _)| (token, count))
        .collect()
}

/// The subject a memory title announces: the text before the first separator.
///
/// Imported memories are titled `<subject> — <detail> · <section> · <claim>`, and
/// that subject is the strongest topic signal in a real vault — far stronger
/// than tags, which are mostly per-source markers such as `unverified`.
fn brain_subject(title: &str) -> Option<String> {
    let head = title.split(['—', '·', '|']).next().unwrap_or(title).trim();
    if head.is_empty() || head.chars().count() > 60 {
        return None;
    }
    // A bare "Reusable knowledge" style heading carries no subject.
    let words = head.split_whitespace().count();
    if words > 6 && !head.contains(":") {
        return None;
    }
    if BRAIN_STOPWORDS.contains(&head.to_lowercase().as_str()) {
        return None;
    }
    let cleaned = head
        .trim_end_matches([',', '.', ':', ';', '-', '，', '。', '：'])
        .trim();
    if cleaned.chars().count() < 3 {
        return None;
    }
    Some(cleaned.to_owned())
}

/// Register a cluster and derive its label, basis, and evidence maps.
fn brain_topic(
    index: usize,
    label: String,
    basis: &'static str,
    members: Vec<usize>,
    entries: &[Entry],
) -> BrainTopic {
    let mut tags = BTreeMap::<String, usize>::new();
    let mut kinds = BTreeMap::<String, usize>::new();
    let mut statuses = BTreeMap::<String, usize>::new();
    for member in &members {
        let entry = &entries[*member];
        *kinds.entry(entry.meta.kind.clone()).or_default() += 1;
        *statuses.entry(entry.meta.status.clone()).or_default() += 1;
        for tag in entry.meta.tags.iter().collect::<BTreeSet<_>>() {
            *tags.entry(tag.clone()).or_default() += 1;
        }
    }
    BrainTopic {
        id: format!("topic:{index}"),
        label,
        basis,
        entries: members,
        tags,
        keywords: BTreeMap::new(),
        kinds,
        statuses,
    }
}

/// Knowledge-graph projection of the vault for the dashboard banner.
///
/// Primary nodes are topics: clusters of memories that already share a subject,
/// a tag, or distinctive keywords. Tags, keywords, and source agents are
/// satellites. Counts describe stored memories only, and an agent node is
/// metadata provenance, never evidence that the agent is currently connected.
async fn brain(State(state): State<UiState>) -> ApiResult<Json<Value>> {
    with_vault(state, brain_payload).await.map(Json)
}

/// The derived relation network, for the relation view.
///
/// Unlike the decorative banner, this is the same graph the CLI and MCP tools
/// serve: typed edges, each with its derivation and the evidence behind it. The
/// view is a window onto it, so legibility limits are applied here and every
/// omission is reported rather than hidden.
async fn graph(
    State(state): State<UiState>,
    Query(query): Query<GraphQuery>,
) -> ApiResult<Json<Value>> {
    let minimum = query.min_weight.clamp(0.0, 1.0);
    let maximum = query.max_nodes.clamp(1, 1000);
    with_vault(state, move |vault| graph_payload(vault, minimum, maximum))
        .await
        .map(Json)
}

#[derive(Debug, Deserialize)]
struct GraphQuery {
    #[serde(default)]
    min_weight: f64,
    #[serde(default = "default_graph_nodes")]
    max_nodes: usize,
}

fn default_graph_nodes() -> usize {
    120
}

/// Build the relation view for one vault snapshot.
fn graph_payload(vault: &Vault, minimum_weight: f64, max_nodes: usize) -> ApiResult<Value> {
    let graph = vault.graph()?;
    let stats = graph.stats(minimum_weight);

    // Keep the strongest edges, then the best-connected nodes, and say what was
    // left out. A truncated picture that admits it is usable; one that does not
    // is a misleading one. Isolated memories sort last and are therefore dropped
    // first — they carry no relations, and the summary reports the count.
    let strong: Vec<crate::graph::Edge> = graph
        .edges
        .iter()
        .filter(|edge| edge.weight >= minimum_weight)
        .cloned()
        .collect();
    let mut ranked: Vec<&crate::graph::Node> = graph.nodes.iter().collect();
    ranked.sort_by(|left, right| {
        right
            .degree
            .cmp(&left.degree)
            .then_with(|| left.id.cmp(&right.id))
    });
    let nodes: Vec<&crate::graph::Node> = ranked.into_iter().take(max_nodes).collect();
    let names: BTreeSet<&str> = nodes.iter().map(|node| node.id.as_str()).collect();
    let edges: Vec<&crate::graph::Edge> = strong
        .iter()
        .filter(|edge| names.contains(edge.from.as_str()) && names.contains(edge.to.as_str()))
        .collect();

    Ok(json!({
        "built_at": graph.built_at,
        "fingerprint": graph.fingerprint,
        "dimensions": graph.dimensions,
        "nodes": nodes,
        "edges": edges,
        "stats": stats,
        "min_weight": minimum_weight,
        "omitted_nodes": graph.nodes.len().saturating_sub(nodes.len()),
        "omitted_edges": strong.len().saturating_sub(edges.len()),
        "notes": graph.notes,
    }))
}

/// Build the knowledge-graph projection for one vault snapshot.
///
/// Separate from the route so it can be measured and asserted without HTTP.
fn brain_payload(vault: &Vault) -> ApiResult<Value> {
    {
        let entries = read_entries(vault)?;
        let now = Utc::now();
        let total = entries.len();

        // Legibility budgets, not data limits: what is left out is reported.
        const MAX_TOPICS: usize = 7;
        const MIN_TOPIC: usize = 2;
        const MAX_TAG_NODES: usize = 26;
        const MAX_KEYWORD_NODES: usize = 20;
        const MAX_AGENT_NODES: usize = 8;
        const MAX_TAGS_PER_TOPIC: usize = 6;
        const MAX_KEYWORDS_PER_TOPIC: usize = 6;
        const MAX_AGENTS_PER_TOPIC: usize = 3;
        const MAX_EXAMPLES: usize = 4;
        const MAX_LABEL_BYTES: usize = 64;

        let document_frequency = token_frequencies(entries.iter());

        // 1. Group by announced subject, then by dominant tag, then by kind.
        let mut by_subject = BTreeMap::<String, Vec<usize>>::new();
        let mut ungrouped = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            match brain_subject(&entry.meta.title) {
                Some(subject) => by_subject.entry(subject).or_default().push(index),
                None => ungrouped.push(index),
            }
        }
        let mut topics = Vec::new();
        let mut singles = Vec::new();
        for (subject, members) in by_subject {
            if members.len() >= MIN_TOPIC {
                topics.push((subject.clone(), "subject", members));
            } else {
                singles.extend(members);
            }
        }
        ungrouped.extend(singles);

        // 2. Whatever is left is grouped by its most common non-marker tag.
        let marker_limit = (total as f64 * 0.5).max(2.0) as usize;
        let tag_frequency = {
            let mut frequency = BTreeMap::<String, usize>::new();
            for entry in &entries {
                for tag in entry.meta.tags.iter().collect::<BTreeSet<_>>() {
                    *frequency.entry(tag.clone()).or_default() += 1;
                }
            }
            frequency
        };
        let mut by_tag = BTreeMap::<String, Vec<usize>>::new();
        let mut leftovers = Vec::new();
        for index in ungrouped {
            let dominant = entries[index]
                .meta
                .tags
                .iter()
                .filter(|tag| tag_frequency[*tag] < marker_limit)
                .max_by(|a, b| {
                    tag_frequency[*a]
                        .cmp(&tag_frequency[*b])
                        .then_with(|| b.cmp(a))
                });
            match dominant {
                Some(tag) => by_tag.entry(tag.clone()).or_default().push(index),
                None => leftovers.push(index),
            }
        }
        for (tag, members) in by_tag {
            if members.len() >= MIN_TOPIC {
                topics.push((tag, "tag", members));
            } else {
                leftovers.extend(members);
            }
        }
        let mut by_kind = BTreeMap::<String, Vec<usize>>::new();
        for index in leftovers {
            by_kind
                .entry(entries[index].meta.kind.clone())
                .or_default()
                .push(index);
        }
        for (kind, members) in by_kind {
            let label = match kind.as_str() {
                "knowledge" => "未归类知识",
                "decision" => "未归类决策",
                "pattern" => "未归类模式",
                "lesson" => "未归类经验",
                "reflection" => "未归类反思",
                other => other,
            };
            topics.push((label.to_owned(), "kind", members));
        }

        topics.sort_by(|a, b| b.2.len().cmp(&a.2.len()).then_with(|| a.0.cmp(&b.0)));
        let omitted_topics = topics.len().saturating_sub(MAX_TOPICS);
        // Truncate before any satellite work: a real vault produces many small
        // clusters (one per title subject), and ranking satellites for all of
        // them costs quadratic time for output that is then thrown away.
        topics.truncate(MAX_TOPICS);

        let mut topics = topics
            .into_iter()
            .enumerate()
            .map(|(index, (label, basis, members))| {
                brain_topic(index, label, basis, members, &entries)
            })
            .collect::<Vec<_>>();
        for topic in &mut topics {
            let members = topic
                .entries
                .iter()
                .map(|index| &entries[*index])
                .collect::<Vec<_>>();
            topic.keywords =
                brain_keywords(&members, total, &document_frequency, MAX_KEYWORDS_PER_TOPIC);
        }

        // 3. Rank every satellite, then attach the strongest to each topic.
        let mut tag_nodes = BTreeMap::<String, BrainNode>::new();
        let mut keyword_nodes = BTreeMap::<String, BrainNode>::new();
        let mut agent_nodes = BTreeMap::<String, BrainNode>::new();
        // One pass over the topic members; a memory contributes once per
        // satellite, and satellite totals come from the counts already computed.
        for topic in &topics {
            for index in &topic.entries {
                let entry = &entries[*index];
                let kind = entry.meta.kind.as_str();
                for tag in entry.meta.tags.iter().collect::<BTreeSet<_>>() {
                    if tag.len() <= MAX_LABEL_BYTES {
                        collect_brain_node(&mut tag_nodes, tag, kind, entry);
                    }
                }
                for agent in entry.meta.source_agents.iter().collect::<BTreeSet<_>>() {
                    if agent.len() <= MAX_LABEL_BYTES {
                        collect_brain_node(&mut agent_nodes, agent, kind, entry);
                    }
                }
            }
            for keyword in topic.keywords.keys() {
                if keyword.len() <= MAX_LABEL_BYTES {
                    for index in &topic.entries {
                        collect_brain_node(
                            &mut keyword_nodes,
                            keyword,
                            &entries[*index].meta.kind,
                            &entries[*index],
                        );
                    }
                }
            }
        }

        // Ranked as owned pairs so the closures below borrow nothing.
        let rank = |values: &BTreeMap<String, usize>, limit: usize| {
            let mut ranked = values
                .iter()
                .map(|(key, value)| (key.clone(), *value))
                .collect::<Vec<_>>();
            ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            ranked.into_iter().take(limit).collect::<Vec<_>>()
        };

        let mut edges = Vec::<Value>::new();
        let mut topic_json = Vec::new();
        let mut weighted_confidence = 0.0f64;
        for topic in &topics {
            let confidence_sum: f64 = topic
                .entries
                .iter()
                .map(|index| entries[*index].meta.effective_confidence_at(now))
                .sum();
            weighted_confidence += confidence_sum;
            let mut ranked = topic.entries.clone();
            ranked.sort_by(|a, b| {
                entries[*b]
                    .meta
                    .effective_confidence_at(now)
                    .total_cmp(&entries[*a].meta.effective_confidence_at(now))
                    .then_with(|| entries[*a].meta.id.cmp(&entries[*b].meta.id))
            });
            let examples = ranked
                .iter()
                .take(MAX_EXAMPLES)
                .map(|index| {
                    json!({
                        "id": entries[*index].meta.id,
                        "title": entries[*index].meta.title,
                        "status": entries[*index].meta.status,
                        "confidence": entries[*index].meta.effective_confidence_at(now),
                    })
                })
                .collect::<Vec<_>>();
            let newest = topic
                .entries
                .iter()
                .map(|index| entries[*index].meta.updated)
                .max()
                .map(|updated| updated.to_rfc3339())
                .unwrap_or_default();
            let tags = rank(&topic.tags, MAX_TAGS_PER_TOPIC)
                .into_iter()
                .map(|(tag, weight)| {
                    edges.push(
                        json!({"from": topic.id, "to": format!("tag:{tag}"), "weight": weight}),
                    );
                    tag
                })
                .collect::<Vec<_>>();
            let keywords = rank(&topic.keywords, MAX_KEYWORDS_PER_TOPIC)
                .into_iter()
                .map(|(keyword, weight)| {
                    edges.push(json!({"from": topic.id, "to": format!("keyword:{keyword}"), "weight": weight}));
                    keyword
                })
                .collect::<Vec<_>>();
            let mut agents_of_topic = BTreeMap::<String, usize>::new();
            for index in &topic.entries {
                for agent in entries[*index]
                    .meta
                    .source_agents
                    .iter()
                    .collect::<BTreeSet<_>>()
                {
                    if agent.len() <= MAX_LABEL_BYTES {
                        *agents_of_topic.entry(agent.clone()).or_default() += 1;
                    }
                }
            }
            let agents = rank(&agents_of_topic, MAX_AGENTS_PER_TOPIC)
                .into_iter()
                .map(|(agent, weight)| {
                    edges.push(
                        json!({"from": topic.id, "to": format!("agent:{agent}"), "weight": weight}),
                    );
                    agent
                })
                .collect::<Vec<_>>();
            topic_json.push(json!({
                "id": topic.id,
                "label": topic.label,
                "basis": topic.basis,
                "count": topic.entries.len(),
                "share": if total == 0 { 0.0 } else { topic.entries.len() as f64 / total as f64 },
                "average_confidence": confidence_sum / topic.entries.len().max(1) as f64,
                "kinds": topic.kinds,
                "statuses": topic.statuses,
                "tags": tags,
                "keywords": keywords,
                "agents": agents,
                "examples": examples,
                "updated": newest,
            }));
        }

        let satellite_json = |nodes: &BTreeMap<String, BrainNode>, group: &str, limit: usize| {
            let mut ranked = nodes.values().collect::<Vec<_>>();
            ranked.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.id.cmp(&b.id)));
            let omitted = ranked.len().saturating_sub(limit);
            let values = ranked
                .iter()
                .take(limit)
                .map(|node| {
                    let mut value = brain_node_json(node);
                    value["type"] = json!(group);
                    value
                })
                .collect::<Vec<_>>();
            (values, omitted)
        };
        let (tag_satellites, hidden_tags) = satellite_json(&tag_nodes, "tag", MAX_TAG_NODES);
        let (keyword_satellites, hidden_keywords) =
            satellite_json(&keyword_nodes, "keyword", MAX_KEYWORD_NODES);
        let (agent_satellites, hidden_agents) =
            satellite_json(&agent_nodes, "agent", MAX_AGENT_NODES);

        let (threshold, schema) = vault
            .config()
            .map(|config| (config.evolution.fading_threshold, config.version))
            .unwrap_or((0.3, 0));
        let stats = json!({
            "total": total,
            "active": entries.iter().filter(|e| e.meta.status == "active").count(),
            "needs_review": entries.iter().filter(|e| {
                matches!(e.meta.status.as_str(), "active" | "fading")
                    && (e.meta.status == "fading"
                        || e.meta.effective_confidence_at(now) < threshold)
            }).count(),
            "average_confidence": if total == 0 { 0.0 } else { weighted_confidence / total as f64 },
        });
        Ok(json!({
            "vault": {
                "name": vault
                    .config()
                    .map(|config| config.vault.name)
                    .unwrap_or_else(|_| "Relic Vault".to_owned()),
                "schema": schema,
            },
            "stats": stats,
            "topics": topic_json,
            "tags": tag_satellites,
            "keywords": keyword_satellites,
            "agents": agent_satellites,
            "edges": edges,
            "truncated": {
                "topics": omitted_topics,
                "tags": hidden_tags,
                "keywords": hidden_keywords,
                "agents": hidden_agents,
            },
        }))
    }
}

#[derive(PartialEq)]
struct IndexedEntry {
    title: String,
    body: String,
    kind: String,
    status: String,
    confidence: f64,
    tags: String,
    source_agents: String,
    path: String,
    updated: String,
}

impl From<&Entry> for IndexedEntry {
    fn from(entry: &Entry) -> Self {
        Self {
            title: entry.meta.title.clone(),
            body: entry.body.clone(),
            kind: entry.meta.kind.clone(),
            status: entry.meta.status.clone(),
            confidence: entry.meta.confidence,
            tags: entry.meta.tags.join(","),
            source_agents: entry.meta.source_agents.join(","),
            path: entry.path.to_string_lossy().into_owned(),
            updated: entry.meta.updated.to_rfc3339(),
        }
    }
}

fn index_health(vault: &Vault, entries: Option<&[Entry]>) -> Value {
    let path = vault.root.join(".relic/index.sqlite");
    if !path.exists() {
        return health(
            "index",
            "warning",
            "Search index is missing; rebuild it from the dashboard",
        );
    }
    let inspect = || -> Result<Value> {
        let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let check: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if check != "ok" {
            bail!("{check}");
        }
        let mut statement = connection.prepare(
            "SELECT id,title,body,kind,status,confidence,tags,source_agents,path,updated FROM entries",
        )?;
        let indexed: BTreeMap<String, IndexedEntry> = statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    IndexedEntry {
                        title: row.get(1)?,
                        body: row.get(2)?,
                        kind: row.get(3)?,
                        status: row.get(4)?,
                        confidence: row.get(5)?,
                        tags: row.get(6)?,
                        source_agents: row.get(7)?,
                        path: row.get(8)?,
                        updated: row.get(9)?,
                    },
                ))
            })?
            .collect::<std::result::Result<_, _>>()?;
        match entries {
            Some(entries)
                if indexed.len() != entries.len()
                    || entries.iter().any(|entry| {
                        indexed.get(&entry.meta.id) != Some(&IndexedEntry::from(entry))
                    }) =>
            {
                Ok(health(
                    "index",
                    "warning",
                    "Search index is out of date; rebuild it to include current memories",
                ))
            }
            Some(_) => Ok(health(
                "index",
                "ok",
                format!("Search index is readable ({} entries)", indexed.len()),
            )),
            None => Ok(health(
                "index",
                "warning",
                "Search index is readable, but memory errors prevent verifying freshness",
            )),
        }
    };
    inspect().unwrap_or_else(|error| {
        health(
            "index",
            "error",
            format!("Search index could not be verified: {error:#}"),
        )
    })
}

#[derive(Default, Deserialize)]
struct EntriesQuery {
    q: Option<String>,
    kind: Option<String>,
    status: Option<String>,
    tag: Option<String>,
    source_agent: Option<String>,
    min_confidence: Option<f64>,
    offset: Option<usize>,
    limit: Option<usize>,
}

/// The one filter the memory list and the distribution view share, so a chart
/// can never describe a different set of memories than the table beneath it.
struct EntryFilter {
    needle: String,
    kind: Option<String>,
    status: Option<String>,
    tag: Option<String>,
    source_agent: Option<String>,
    min_confidence: Option<f64>,
}

impl EntryFilter {
    fn from_query(query: &EntriesQuery) -> Self {
        let clean = |value: &Option<String>| {
            value
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        Self {
            needle: query.q.as_deref().unwrap_or_default().trim().to_lowercase(),
            kind: clean(&query.kind),
            status: clean(&query.status),
            tag: clean(&query.tag),
            source_agent: clean(&query.source_agent),
            min_confidence: query.min_confidence,
        }
    }

    fn matches(&self, entry: &Entry, now: DateTime<Utc>) -> bool {
        (self.needle.is_empty()
            || entry.meta.title.to_lowercase().contains(&self.needle)
            || entry.body.to_lowercase().contains(&self.needle)
            || entry.meta.id.to_lowercase().contains(&self.needle)
            || entry
                .meta
                .tags
                .iter()
                .any(|tag| tag.to_lowercase().contains(&self.needle)))
            && self.kind.as_ref().is_none_or(|v| &entry.meta.kind == v)
            && self.status.as_ref().is_none_or(|v| &entry.meta.status == v)
            && self
                .tag
                .as_ref()
                .is_none_or(|v| entry.meta.tags.contains(v))
            && self
                .source_agent
                .as_ref()
                .is_none_or(|v| entry.meta.source_agents.contains(v))
            && self
                .min_confidence
                .is_none_or(|v| entry.meta.effective_confidence_at(now) >= v)
    }

    fn is_scoped(&self) -> bool {
        !self.needle.is_empty()
            || self.min_confidence.is_some()
            || self.kind.is_some()
            || self.status.is_some()
            || self.tag.is_some()
            || self.source_agent.is_some()
    }
}

async fn entries(
    State(state): State<UiState>,
    query: std::result::Result<Query<EntriesQuery>, QueryRejection>,
) -> ApiResult<Json<Value>> {
    let Query(query) = query.map_err(|error| ApiError::invalid(error.body_text()))?;
    if let Some(confidence) = query.min_confidence {
        validate_confidence(confidence)?;
    }
    let filter = EntryFilter::from_query(&query);
    with_vault(state, move |vault| {
        let all = read_entries(vault)?;
        let tags: BTreeSet<_> = all
            .iter()
            .flat_map(|e| e.meta.tags.iter().cloned())
            .collect();
        let agents: BTreeSet<_> = all
            .iter()
            .flat_map(|e| e.meta.source_agents.iter().cloned())
            .collect();
        let now = Utc::now();
        let matches: Vec<_> = all
            .iter()
            .filter(|entry| filter.matches(entry, now))
            .collect();
        let selected: Vec<_> = matches
            .iter()
            .skip(query.offset.unwrap_or(0))
            .take(query.limit.unwrap_or(50).clamp(1, 200))
            .map(|entry| entry_json(vault, entry))
            .collect::<Result<Vec<_>>>()?;
        Ok(Json(
            json!({ "entries": selected, "total": matches.len(), "tags": tags, "agents": agents }),
        ))
    })
    .await
}

/// Histogram resolution and the confidence cut-offs the dashboard reports.
const CONFIDENCE_BINS: usize = 10;
const CONFIDENCE_LEVELS: [f64; 4] = [0.3, 0.5, 0.7, 0.9];
/// More memories justify a tighter kernel; a handful would otherwise draw a
/// spiky line that reads as structure that is not there.
const MIN_BANDWIDTH: f64 = 0.05;
const MAX_BANDWIDTH: f64 = 0.14;

/// Gaussian kernel density on the closed confidence range, evaluated at the
/// histogram edges. Trust is not uniform inside a bin, so the density is the
/// honest shape; the bin counts stay alongside it as observations.
fn density_at(points: &[f64], bandwidth: f64, x: f64) -> f64 {
    let total = points.len() as f64;
    if total == 0.0 {
        return 0.0;
    }
    let sum = points
        .iter()
        .map(|point| {
            let z = (x - point) / bandwidth;
            (-0.5 * z * z).exp()
        })
        .sum::<f64>();
    sum / (total * bandwidth * (2.0 * std::f64::consts::PI).sqrt())
}

fn distribution(values: &BTreeMap<String, usize>) -> Value {
    Value::Array(
        values
            .iter()
            .map(|(name, count)| json!({ "name": name, "count": count }))
            .collect(),
    )
}

fn distribution_from<'a, I>(
    entries: I,
    values: impl Fn(&'a Entry) -> &'a [String],
) -> BTreeMap<String, usize>
where
    I: Iterator<Item = &'a Entry>,
{
    // One entry counts once per distinct tag or agent, so a repeated label in a
    // single memory cannot inflate the chart.
    let mut counts = BTreeMap::<String, usize>::new();
    for entry in entries {
        for value in values(entry).iter().collect::<BTreeSet<_>>() {
            *counts.entry(value.clone()).or_default() += 1;
        }
    }
    counts
}

/// Distribution view for the memory library.
///
/// It answers the same filters as `GET /api/entries`, so every chart describes
/// exactly the memories the list below it shows. Confidence is always the
/// effective value (decay and expiry applied); stored confidences would let a
/// stale memory look trustworthy in a chart.
async fn stats(
    State(state): State<UiState>,
    query: std::result::Result<Query<EntriesQuery>, QueryRejection>,
) -> ApiResult<Json<Value>> {
    let Query(query) = query.map_err(|error| ApiError::invalid(error.body_text()))?;
    if let Some(confidence) = query.min_confidence {
        validate_confidence(confidence)?;
    }
    let filter = EntryFilter::from_query(&query);
    with_vault(state, move |vault| {
        let entries = read_entries(vault)?;
        let now = Utc::now();
        let scoped: Vec<&Entry> = entries
            .iter()
            .filter(|entry| filter.matches(entry, now))
            .collect();
        let confidence_of = |entry: &Entry| entry.meta.effective_confidence_at(now);

        let mut by_kind = BTreeMap::<String, usize>::new();
        let mut by_status = BTreeMap::<String, usize>::new();
        let mut bins = [0usize; CONFIDENCE_BINS];
        let mut values = Vec::with_capacity(scoped.len());
        for entry in &scoped {
            *by_kind.entry(entry.meta.kind.clone()).or_default() += 1;
            *by_status.entry(entry.meta.status.clone()).or_default() += 1;
            let value = confidence_of(entry);
            values.push(value);
            // A confidence of 1.0 belongs in the last bin, not in an eleventh.
            let index = ((value * CONFIDENCE_BINS as f64) as usize).min(CONFIDENCE_BINS - 1);
            bins[index] += 1;
        }
        let by_tag = distribution_from(scoped.iter().copied(), |entry| &entry.meta.tags);
        let by_agent = distribution_from(scoped.iter().copied(), |entry| &entry.meta.source_agents);

        let total = scoped.len();
        let (mean, standard_deviation, median) = if total == 0 {
            (0.0, 0.0, 0.0)
        } else {
            let mean = values.iter().sum::<f64>() / total as f64;
            let variance = values
                .iter()
                .map(|value| (value - mean).powi(2))
                .sum::<f64>()
                / total as f64;
            let mut sorted = values.clone();
            sorted.sort_by(f64::total_cmp);
            (mean, variance.sqrt(), sorted[total / 2])
        };
        let bandwidth = (1.0 / (total as f64).sqrt() * 0.75).clamp(MIN_BANDWIDTH, MAX_BANDWIDTH);
        let bin_width = 1.0 / CONFIDENCE_BINS as f64;
        let confidence_distribution = (0..CONFIDENCE_BINS)
            .map(|index| {
                let from = index as f64 * bin_width;
                let to = from + bin_width;
                json!({
                    "label": format!("{from:.1}–{to:.1}"),
                    "from": from,
                    "to": to,
                    "count": bins[index],
                    "density": density_at(&values, bandwidth, to),
                })
            })
            .collect::<Vec<_>>();
        let thresholds = CONFIDENCE_LEVELS
            .iter()
            .map(|threshold| {
                json!({
                    "level": threshold,
                    "above": values.iter().filter(|value| **value >= *threshold).count(),
                    "below": values.iter().filter(|value| **value < *threshold).count(),
                })
            })
            .collect::<Vec<_>>();
        // The threshold the vault itself uses to flag fading memories, so the
        // view can point at the cut-off that actually governs review.
        let decay_threshold = vault
            .config()
            .map(|config| config.evolution.fading_threshold)
            .unwrap_or(0.3);
        // A label that is too long makes a chart axis unreadable; the brain
        // banner drops the same values for the same reason.
        let truncated = by_tag.keys().chain(by_agent.keys()).filter(|name| name.len() > 64).count();
        Ok(Json(json!({
            "scope": {
                "filtered": filter.is_scoped(),
                "total": total,
                "vault_total": entries.len(),
            },
            "distribution": {
                "by_kind": distribution(&by_kind),
                "by_status": distribution(&by_status),
                "by_tag": distribution(&by_tag),
                "by_agent": distribution(&by_agent),
            },
            "confidence": {
                "mean": mean,
                "median": median,
                "standard_deviation": standard_deviation,
                "bandwidth": bandwidth,
                "bins": confidence_distribution,
                "thresholds": thresholds,
                "decay_threshold": decay_threshold,
            },
            "labels": { "tags": by_tag.keys().cloned().collect::<Vec<_>>(), "agents": by_agent.keys().cloned().collect::<Vec<_>>() },
            "truncated": { "labels": truncated },
        })))
    })
    .await
}

async fn get_entry(State(state): State<UiState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    with_vault(state, move |vault| {
        let entry = find_entry(vault, &id)?;
        Ok(Json(json!({ "entry": entry_json(vault, &entry)? })))
    })
    .await
}

#[derive(Deserialize)]
struct CreateEntry {
    title: String,
    #[serde(default)]
    content: String,
    kind: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    confidence: Option<f64>,
    source_agent: Option<String>,
}

fn validate_confidence(confidence: f64) -> ApiResult<()> {
    if !(0.0..=1.0).contains(&confidence) {
        return Err(ApiError::invalid("confidence must be between 0 and 1"));
    }
    Ok(())
}

fn validate_title(title: &str) -> ApiResult<()> {
    if title.trim().is_empty() {
        return Err(ApiError::invalid("title cannot be empty"));
    }
    if title.len() > 240 {
        return Err(ApiError::invalid("title cannot exceed 240 UTF-8 bytes"));
    }
    Ok(())
}

async fn create_entry(
    State(state): State<UiState>,
    request: std::result::Result<Json<CreateEntry>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let Json(request) = request.map_err(json_error)?;
    validate_title(&request.title)?;
    if let Some(confidence) = request.confidence {
        validate_confidence(confidence)?;
    }
    let kind = request.kind.unwrap_or_else(|| "knowledge".into());
    if !["knowledge", "pattern", "lesson", "decision", "reflection"].contains(&kind.as_str()) {
        return Err(ApiError::invalid(format!(
            "Unsupported memory kind '{kind}'"
        )));
    }
    with_vault(state, move |vault| {
        // Do not write a new entry when existing malformed files would make
        // Vault::create's index rebuild fail after the Markdown was saved.
        read_entries(vault)?;
        let config = vault.config()?;
        let entry = vault.create(
            &request.title,
            &request.content,
            &kind,
            request.tags,
            request
                .confidence
                .unwrap_or(config.vault.default_confidence),
            request.source_agent.as_deref().unwrap_or("relic-ui"),
        )?;
        Ok((
            StatusCode::CREATED,
            Json(json!({ "entry": entry_json(vault, &entry)? })),
        ))
    })
    .await
}

#[derive(Deserialize)]
struct UpdateEntry {
    title: Option<String>,
    content: Option<String>,
    status: Option<String>,
    confidence: Option<f64>,
    tags: Option<Vec<String>>,
    expected_updated: Option<DateTime<Utc>>,
    expected_revision: Option<String>,
}

async fn update_entry(
    State(state): State<UiState>,
    Path(id): Path<String>,
    request: std::result::Result<Json<UpdateEntry>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let Json(request) = request.map_err(json_error)?;
    if let Some(title) = &request.title {
        validate_title(title)?;
    }
    if let Some(confidence) = request.confidence {
        validate_confidence(confidence)?;
    }
    if request.status.as_ref().is_some_and(|status| {
        !["active", "fading", "superseded", "archived"].contains(&status.as_str())
    }) {
        return Err(ApiError::invalid("Unsupported memory status"));
    }
    with_vault(state, move |vault| {
        let current = find_entry(vault, &id)?;
        if let Some(expected) = request.expected_revision {
            let raw =
                fs::read_to_string(&current.path).context("could not check memory revision")?;
            if expected != revision(&raw) {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "This memory file changed since it was opened. Reload it before saving.",
                ));
            }
        }
        if request
            .expected_updated
            .is_some_and(|revision| revision != current.meta.updated)
        {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "This memory changed since it was opened. Reload it before saving.",
            ));
        }
        let entry = vault.update(
            &id,
            EntryPatch {
                title: request.title,
                content: request.content,
                status: request.status,
                confidence: request.confidence,
                tags: request.tags,
                ..EntryPatch::default()
            },
        )?;
        Ok(Json(json!({ "entry": entry_json(vault, &entry)? })))
    })
    .await
}

fn config_value(vault: &Vault) -> Value {
    match fs::read_to_string(vault.root.join(".relic/config.yaml")) {
        Ok(raw) => {
            let parsed = parse_config(&raw);
            match parsed {
                Ok(config) => json!({ "raw": raw, "config": config, "error": null }),
                Err(error) => json!({ "raw": raw, "config": null, "error": format!("{error:#}") }),
            }
        }
        Err(error) => {
            json!({ "raw": "", "config": null, "error": format!("Could not read config: {error}") })
        }
    }
}

fn parse_config(raw: &str) -> Result<Config> {
    let config: Config = serde_yaml::from_str(raw).context("invalid vault config")?;
    config.validate()?;
    Ok(config)
}

async fn get_config(State(state): State<UiState>) -> ApiResult<Json<Value>> {
    with_vault(state, |vault| Ok(Json(config_value(vault)))).await
}

#[derive(Deserialize)]
struct UpdateConfig {
    raw: String,
    expected_raw: String,
}

async fn update_config(
    State(state): State<UiState>,
    request: std::result::Result<Json<UpdateConfig>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let Json(request) = request.map_err(json_error)?;
    parse_config(&request.raw).map_err(|error| ApiError::invalid(format!("{error:#}")))?;
    with_vault(state, move |vault| {
        let path = vault.root.join(".relic/config.yaml");
        let current = fs::read_to_string(&path).context("could not read the current config")?;
        if current != request.expected_raw {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "Configuration changed since it was opened. Reload it before saving.",
            ));
        }
        // The raw YAML is the contract: serialization would discard comments
        // and unknown settings. Rename a sibling temporary file to avoid a
        // partially written config if the process exits while saving.
        let temporary = path.with_file_name(format!(".config-{}.tmp", uuid::Uuid::new_v4()));
        let save = || -> Result<()> {
            use std::io::Write;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.set_permissions(fs::metadata(&path)?.permissions())?;
            file.write_all(request.raw.as_bytes())?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, &path)?;
            Ok(())
        };
        let outcome = save();
        if outcome.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        outcome.context("could not save vault config")?;
        Ok(Json(config_value(vault)))
    })
    .await
}

async fn reindex(State(state): State<UiState>) -> ApiResult<Json<Value>> {
    with_vault(state, |vault| {
        read_entries(vault)?;
        Ok(Json(json!({ "count": vault.reindex()? })))
    })
    .await
}

async fn automation_status(State(state): State<UiState>) -> ApiResult<Json<Value>> {
    with_vault(state, |v| Ok(Json(crate::automation::status(v)?))).await
}
async fn automation_retry(State(state): State<UiState>) -> ApiResult<Json<Value>> {
    with_vault(state, |v| {
        crate::automation::retry_sync(v)?;
        Ok(Json(json!({"scheduled":true})))
    })
    .await
}
async fn captures_work(State(state): State<UiState>) -> ApiResult<Json<Value>> {
    with_vault(state, |v| {
        Ok(Json(
            serde_json::to_value(crate::capture::work(v, 100)?).map_err(anyhow::Error::from)?,
        ))
    })
    .await
}
async fn captures_review(
    State(state): State<UiState>,
    Path(id): Path<uuid::Uuid>,
    body: Result<Json<crate::capture::Review>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let Json(review) = body.map_err(json_error)?;
    with_vault(state, move |v| {
        Ok(Json(
            serde_json::to_value(crate::capture::review(v, id, review)?)
                .map_err(anyhow::Error::from)?,
        ))
    })
    .await
}
async fn captures_retry(
    State(state): State<UiState>,
    Path(id): Path<uuid::Uuid>,
) -> ApiResult<Json<Value>> {
    with_vault(state, move |v| {
        Ok(Json(
            serde_json::to_value(crate::capture::retry(v, id)?).map_err(anyhow::Error::from)?,
        ))
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupRequest {
    host: String,
    project: std::path::PathBuf,
    dsh_home: std::path::PathBuf,
}
async fn automation_setup(
    State(state): State<UiState>,
    body: Result<Json<SetupRequest>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let Json(request) = body.map_err(json_error)?;
    with_vault(state, move |v| {
        match request.host.as_str() {
            "worker" => {
                crate::automation::start(v, &std::env::current_exe().map_err(anyhow::Error::from)?)?
            }
            "stop" => crate::automation::stop(v)?,
            "codex" | "dsh" => crate::setup::install(
                v,
                &request.project,
                &request.dsh_home,
                &std::env::current_exe().map_err(anyhow::Error::from)?,
                &request.host,
            )?,
            _ => return Err(ApiError::invalid("Unsupported setup action")),
        }
        Ok(Json(crate::automation::status(v)?))
    })
    .await
}

async fn history_defaults() -> Json<Value> {
    Json(crate::history::defaults())
}
async fn history_scan(
    State(state): State<UiState>,
    body: Result<Json<crate::history::ScanRequest>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let Json(request) = body.map_err(json_error)?;
    with_vault(state, move |v| {
        Ok(Json(
            crate::history::scan(v, request).map_err(|e| ApiError::invalid(e.to_string()))?,
        ))
    })
    .await
}
async fn history_import(
    State(state): State<UiState>,
    body: Result<Json<crate::history::ImportRequest>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let Json(request) = body.map_err(json_error)?;
    with_vault(state, move |v| {
        Ok(Json(
            serde_json::to_value(
                crate::history::import(v, request).map_err(|e| ApiError::invalid(e.to_string()))?,
            )
            .map_err(anyhow::Error::from)?,
        ))
    })
    .await
}

async fn history_prepare(
    State(state): State<UiState>,
    Path(id): Path<uuid::Uuid>,
) -> ApiResult<Json<Value>> {
    with_vault(state, move |v| {
        let (source, _) = crate::capture::get(v, id)?;
        if !source.input.tags.iter().any(|t| t == "manual-history") {
            return Err(ApiError::invalid("不是历史提炼候选"));
        }
        let report = crate::capture::work_one(v, id)?;
        let (_, item) = crate::capture::get(v, id)?;
        Ok(Json(json!({"report":report,"item":item})))
    })
    .await
}
