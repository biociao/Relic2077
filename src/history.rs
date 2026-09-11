//! Explicit, local-only history discovery. Never called by hooks or the daemon.
//! Only user text and completed final replies become candidate sources.
use crate::{
    capture::{self, CaptureInput, QueueItem, Status},
    vault::Vault,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

const MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FILES: usize = 1000;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanRequest {
    pub host: String,
    pub root: PathBuf,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    pub host: String,
    pub root: PathBuf,
    pub file: PathBuf,
    pub fingerprint: String,
}
#[derive(Serialize)]
pub struct SessionRow {
    pub file: PathBuf,
    pub session_id: String,
    pub project: String,
    pub title: String,
    pub fingerprint: String,
    pub turns: usize,
    pub status: String,
    pub detail: String,
    pub captures: Vec<QueueItem>,
    pub entries: Vec<String>,
}
struct Turn {
    id: String,
    prompt: String,
    response: String,
}
struct Parsed {
    session_id: String,
    project: String,
    title: String,
    fingerprint: String,
    turns: Vec<Turn>,
    write_seen: bool,
    incomplete: bool,
    forked: bool,
}
fn field<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn bounded(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
fn text(v: &Value) -> String {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter(|b| matches!(field(b, "type"), "text" | "input_text" | "output_text"))
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}
pub(crate) fn has_session(body: &str, id: &str) -> bool {
    body.lines()
        .any(|line| line.strip_prefix("- Session: ") == Some(id))
}
pub fn pair_key(prompt: &str, response: &str) -> String {
    let bytes = serde_json::to_vec(&(bounded(prompt, 8000), bounded(response, 24000)))
        .expect("strings serialize");
    format!("history-pair:{:x}", Sha256::digest(bytes))
}
fn validate_root(request: &ScanRequest) -> Result<PathBuf> {
    ensure!(
        matches!(request.host.as_str(), "codex" | "dsh"),
        "不支持的会话来源"
    );
    ensure!(request.root.is_absolute(), "请选择会话目录的绝对路径");
    let root = fs::canonicalize(&request.root).context("无法打开会话目录")?;
    ensure!(root.is_dir(), "会话目录不存在");
    Ok(root)
}
fn supported(path: &Path, host: &str) -> bool {
    path.extension().is_some_and(|s| s == "jsonl")
        || (host == "dsh" && path.file_name().is_some_and(|s| s == "session.jsonl.zstd"))
}
fn parse(path: &Path, host: &str) -> Result<Parsed> {
    let file = File::open(path)?;
    ensure!(
        file.metadata()?.len() <= MAX_BYTES,
        "会话文件超过 64 MiB，请先拆分或使用专门导入工具"
    );
    let reader: Box<dyn Read> = if path.extension().is_some_and(|s| s == "zstd") {
        Box::new(zstd::stream::read::Decoder::new(file)?)
    } else {
        Box::new(file)
    };
    let mut reader = BufReader::new(reader.take(MAX_BYTES + 1));
    let mut digest = Sha256::new();
    let mut used = 0;
    let mut parsed = Parsed {
        session_id: String::new(),
        project: String::new(),
        title: String::new(),
        fingerprint: String::new(),
        turns: vec![],
        write_seen: false,
        incomplete: false,
        forked: false,
    };
    let mut prompt = String::new();
    let mut turn_id = String::new();
    let mut response = String::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        let size = reader.read_until(b'\n', &mut line)?;
        if size == 0 {
            break;
        }
        used += size as u64;
        ensure!(used <= MAX_BYTES, "解压后的会话超过 64 MiB");
        digest.update(&line);
        let value: Value = serde_json::from_slice(&line)
            .context("会话记录不完整或格式不支持，请等待会话停止写入")?;
        let kind = field(&value, "type");
        let p = &value["payload"];
        let d = &value["data"];
        // Conservative legacy guard: a write invocation is not proof of success.
        // Do not import it automatically when older entries lack session provenance.
        if (host == "codex"
            && kind == "response_item"
            && matches!(field(p, "type"), "function_call" | "custom_tool_call"))
            || (host == "dsh" && kind == "tool/call")
        {
            let call = if host == "codex" { p } else { d }.to_string();
            if [
                "relic_create_entry",
                "relic_update_entry",
                "relic_review_capture",
                "relic_capture",
                "relic create",
                "relic capture",
            ]
            .iter()
            .any(|name| call.contains(name))
            {
                parsed.write_seen = true;
            }
        }
        if host == "codex" {
            match kind {
                "session_meta" => {
                    parsed.forked = p.get("parent_thread_id").is_some_and(|v| !v.is_null());
                    parsed.session_id = field(p, "id").to_owned();
                    if parsed.session_id.is_empty() {
                        parsed.session_id = field(p, "session_id").to_owned();
                    }
                    parsed.project = field(p, "cwd").to_owned();
                }
                "event_msg" => match field(p, "type") {
                    "task_started" => {
                        turn_id = field(p, "turn_id").to_owned();
                        response.clear();
                        prompt.clear();
                        parsed.incomplete = true;
                    }
                    "user_message" => {
                        prompt = field(p, "message").to_owned();
                    }
                    "task_complete" => {
                        let final_text = field(p, "last_agent_message");
                        if !final_text.is_empty() {
                            response = final_text.to_owned();
                        }
                        if turn_id.is_empty() {
                            turn_id = field(p, "turn_id").to_owned();
                        }
                        if !response.trim().is_empty() && !turn_id.is_empty() {
                            parsed.turns.push(Turn {
                                id: turn_id.clone(),
                                prompt: bounded(&prompt, 8000),
                                response: bounded(&response, 24000),
                            });
                        }
                        parsed.incomplete = false;
                        response.clear();
                    }
                    "turn_aborted" => {
                        parsed.incomplete = false;
                        response.clear();
                    }
                    _ => {}
                },
                "response_item" if field(p, "type") == "message" => {
                    if field(p, "role") == "user" {
                        prompt = text(&p["content"]);
                    }
                    if field(p, "role") == "assistant"
                        && matches!(field(p, "phase"), "final_answer" | "final")
                    {
                        response = text(&p["content"]);
                    }
                }
                _ => {}
            }
        } else {
            match kind {
                "session" => {
                    parsed.session_id = field(&value, "id").to_owned();
                    parsed.project = field(&value, "cwd").to_owned();
                }
                "session/title" => {
                    parsed.title = bounded(field(d, "title"), 100);
                }
                "turn/start" => {
                    turn_id = d["turn"].to_string();
                    response.clear();
                    parsed.incomplete = true;
                }
                "user/message" if field(&d["source"], "kind") != "plugin" => {
                    prompt = text(&d["content"]);
                }
                "assistant/message" if d["interrupted"] != true => {
                    if d["turn"]
                        .as_u64()
                        .is_some_and(|t| turn_id.parse::<u64>() == Ok(t))
                    {
                        response = text(&d["message"]["content"]);
                    }
                }
                "turn/end" => {
                    if field(&d["reason"], "kind") == "completed"
                        && d["turn"]
                            .as_u64()
                            .is_some_and(|t| turn_id.parse::<u64>() == Ok(t))
                        && !response.trim().is_empty()
                        && !turn_id.is_empty()
                    {
                        parsed.turns.push(Turn {
                            id: turn_id.clone(),
                            prompt: bounded(&prompt, 8000),
                            response: bounded(&response, 24000),
                        });
                    }
                    parsed.incomplete = false;
                    response.clear();
                }
                _ => {}
            }
        }
    }
    ensure!(
        !parsed.session_id.is_empty() && !parsed.project.is_empty(),
        "缺少会话 ID 或项目路径，不能可靠去重"
    );
    ensure!(parsed.session_id.len() <= 256, "会话 ID 过长");
    if parsed.title.is_empty() {
        parsed.title = bounded(
            parsed
                .turns
                .first()
                .map(|t| t.prompt.as_str())
                .unwrap_or("无已完成轮次"),
            100,
        );
    }
    parsed.fingerprint = format!("{:x}", digest.finalize());
    Ok(parsed)
}
fn row(vault: &Vault, host: &str, file: PathBuf, parsed: &Parsed) -> Result<SessionRow> {
    let mut captures = vec![];
    for item in capture::list(vault)? {
        let (source, _) = capture::get(vault, item.id)?;
        if source.input.source_agent == host && source.input.session_id == parsed.session_id {
            captures.push(item);
        }
    }
    let entries: Vec<String> = vault
        .entries()?
        .into_iter()
        .filter(|e| {
            e.body
                .contains(&format!("- Session: {}", parsed.session_id))
        })
        .map(|e| e.meta.id)
        .collect();
    let (status, detail) =
        if !entries.is_empty() || captures.iter().any(|q| q.status == Status::Accepted) {
            ("recorded", "已关联正式记忆，不重复提炼")
        } else if !captures.is_empty() {
            (
                "captured",
                "已有采集记录（含待审核、失败或拒绝），请处理原候选",
            )
        } else if parsed.forked {
            (
                "check_required",
                "此会话来自分支，可能包含已处理的父会话历史；请先核对，暂不导入",
            )
        } else if parsed.write_seen {
            (
                "check_required",
                "检测到 Relic 写入调用；旧记录可能缺少会话关联。请核对原会话，暂不导入",
            )
        } else if parsed.incomplete {
            ("active", "会话存在未完成或中断轮次，暂不导入")
        } else if parsed.turns.is_empty() {
            ("empty", "没有可识别的已完成轮次")
        } else {
            ("available", "未发现关联采集，可手动生成待审核候选")
        };
    Ok(SessionRow {
        file,
        session_id: parsed.session_id.clone(),
        project: parsed.project.clone(),
        title: parsed.title.clone(),
        fingerprint: parsed.fingerprint.clone(),
        turns: parsed.turns.len(),
        status: status.into(),
        detail: detail.into(),
        captures,
        entries,
    })
}
pub fn defaults() -> Value {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default();
    let codex = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    let dsh = std::env::var_os("DSH_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".dsh"));
    json!({"codex":codex.join("sessions"),"dsh":dsh.join("sessions")})
}
pub fn scan(vault: &Vault, request: ScanRequest) -> Result<Value> {
    let root = validate_root(&request)?;
    let mut files = vec![];
    let mut errors = vec![];
    let mut visited = 0;
    for item in WalkDir::new(&root)
        .follow_links(false)
        .max_depth(10)
        .into_iter()
        .take(20_000)
    {
        visited += 1;
        match item {
            Ok(e) if e.file_type().is_file() && supported(e.path(), &request.host) => {
                files.push(e.into_path())
            }
            Err(_) => errors.push("部分目录无法读取".to_owned()),
            _ => {}
        }
    }
    files.sort_by_key(|p| std::cmp::Reverse(fs::metadata(p).and_then(|m| m.modified()).ok()));
    let truncated = files.len() > MAX_FILES || visited == 20_000;
    let mut sessions = BTreeMap::new();
    for file in files.into_iter().take(MAX_FILES) {
        let relative = file.strip_prefix(&root)?.to_path_buf();
        match parse(&file, &request.host)
            .and_then(|parsed| row(vault, &request.host, relative.clone(), &parsed))
        {
            Ok(item) => {
                sessions.entry(item.session_id.clone()).or_insert(item);
            }
            Err(e) => errors.push(format!("{}：{e}", relative.display())),
        }
    }
    Ok(
        json!({"sessions":sessions.into_values().collect::<Vec<_>>(),"errors":errors,"truncated":truncated,"backend":crate::distillation::config(vault)?}),
    )
}
pub fn import(vault: &Vault, request: ImportRequest) -> Result<QueueItem> {
    let root = validate_root(&ScanRequest {
        host: request.host.clone(),
        root: request.root,
    })?;
    ensure!(
        !request.file.is_absolute()
            && request
                .file
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
        "无效会话路径"
    );
    let file = fs::canonicalize(root.join(&request.file))?;
    ensure!(
        file.starts_with(&root) && supported(&file, &request.host),
        "会话文件不在所选目录内"
    );
    let parsed = parse(&file, &request.host)?;
    ensure!(
        parsed.fingerprint == request.fingerprint,
        "会话在扫描后已改变，请重新扫描再选择"
    );
    let input = make_input(&request.host, &parsed)?;
    // Retry of the same manually selected snapshot is handled under the capture lock.
    let old = capture::list(vault)?
        .into_iter()
        .find(|q| capture::get(vault, q.id).is_ok_and(|(s, _)| s.input == input));
    if let Some(old) = old {
        return Ok(old);
    }
    let state = row(vault, &request.host, request.file, &parsed)?;
    ensure!(state.status == "available", "{}", state.detail);
    capture::capture_history(vault, input)
}
fn make_input(host: &str, p: &Parsed) -> Result<CaptureInput> {
    let mut outcome = String::new();
    let mut evidence = vec![format!("history-snapshot:{}", p.fingerprint)];
    for t in &p.turns {
        outcome.push_str(&format!(
            "\n## 轮次 {}\n用户：\n{}\n最终回复：\n{}\n",
            t.id, t.prompt, t.response
        ));
        evidence.push(format!("history-turn:turn:{}", t.id));
        evidence.push(pair_key(&t.prompt, &t.response));
    }
    ensure!(
        outcome.len() <= 180_000,
        "会话内容超过单次提炼上限（180 KB）；未导入，请使用分段提炼工具"
    );
    Ok(CaptureInput {
        event_id: "history:v1".into(),
        project: p.project.clone(),
        session_id: p.session_id.clone(),
        source_agent: host.into(),
        title: format!("历史会话 · {}", p.title),
        context: format!(
            "用户手动选择的 {host} 历史会话，共 {} 个完成轮次。仅包含用户文本和最终回复，各轮最多 8,000 / 24,000 字符；事实与证据尚未验证。history-* 标识只用于来源追踪和去重，不是支持结论的证据。",
            p.turns.len()
        ),
        action: "手动历史提炼：整理可复用经验供审核，不自动写入正式记忆。".into(),
        outcome,
        evidence,
        tags: vec!["manual-history".into(), "unverified".into()],
    })
}
