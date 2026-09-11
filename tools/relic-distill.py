#!/usr/bin/env python3
"""relic-distill — a Relic distiller backend that does not need Relic to ship a model.

Relic's distillation contract is a command that reads one request JSON object on
stdin and writes one output JSON object on stdout (`src/distillation.rs`):

  < {"version":1,"instructions":"…","source":{…},"related_knowledge":[…]}
  > {"knowledge":{"title","content","kind","confidence","tags":[]},
     "recommendation":"new|update|skip","rationale":"…",
     "evidence_indices":[],"related_entry_ids":[]}

This script fills that contract from whatever model channel the device has, in
order of preference, and always returns a schema-valid object — a distiller that
crashes leaves the queue stuck, which is worse than a plain draft:

  1. RELIC_DISTILL_CMD      the model command. Put {prompt} where the prompt
                            goes as one argument, or omit it to pipe the prompt
                            on stdin. Defaults to the DSH headless profile on
                            this device ("dsh --profile headless {prompt}",
                            ~2s per call), which needs no extra API key.
  2. RELIC_DISTILL_BASE_URL an OpenAI-compatible /chat/completions endpoint
                            (+ RELIC_DISTILL_API_KEY, RELIC_DISTILL_MODEL)
  3. built-in extractive fallback: no model, deterministic, never fails

Configure it once:

  cat > /tmp/distillation.json <<'JSON'
  {"backend":"command","executable":"/abs/path/relic-distill.py",
   "args":[],"timeout_seconds":60}
  JSON
  relic distiller configure /tmp/distillation.json

Env knobs: RELIC_DISTILL_CMD, RELIC_DISTILL_BASE_URL, RELIC_DISTILL_API_KEY,
RELIC_DISTILL_MODEL, RELIC_DISTILL_TIMEOUT, RELIC_DISTILL_DEBUG=1.
"""

from __future__ import annotations

import json
import os
import pathlib
import re
import shlex
import shutil
import subprocess
import sys
import urllib.error
import urllib.request

KINDS = ("knowledge", "lesson", "decision", "pattern")
NAMESPACE = ("src:", "project:", "section:", "pool:")

PROMPT_TEMPLATE = """{instructions}

## Source experience
event_id: {event_id}
project: {project}
source_agent: {source_agent}
title: {title}

### context
{context}

### action
{action}

### outcome
{outcome}

### evidence
{evidence}

## Related knowledge already in the vault
{related}

## Subject tags this vault already uses
{vocabulary}

Reuse the tags above whenever one fits; invent a new subject tag only when none
does. Do not tag a whole batch with one shared label — scoping tags (src:,
project:, section:, pool:) are added for you and must not be treated as subjects.

Answer with ONE JSON object and nothing else:
{{"knowledge":{{"title":"...","content":"...","kind":"knowledge|lesson|decision|pattern","confidence":0.0,"tags":["..."]}},"recommendation":"new|update|skip","rationale":"...","evidence_indices":[],"related_entry_ids":[]}}
"""


# --------------------------------------------------------------- model channels

def default_command() -> str:
    """The device channel when nothing is configured: DSH's headless profile."""
    if shutil.which("dsh"):
        return "dsh --profile headless {prompt}"
    return ""


def via_command(prompt: str, timeout: float) -> str | None:
    command = os.environ.get("RELIC_DISTILL_CMD", "").strip() or default_command()
    if not command:
        return None
    argv = shlex.split(command)
    # "{prompt}" passes the prompt as one argv element (no shell, no quoting
    # limits), which is what a CLI like `dsh --profile headless` needs. Without
    # the placeholder the prompt is piped on stdin instead.
    if "{prompt}" in argv:
        argv = [prompt if part == "{prompt}" else part for part in argv]
        stdin = None
    else:
        stdin = prompt
    try:
        done = subprocess.run(
            argv,
            input=stdin,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except (subprocess.TimeoutExpired, OSError) as exc:
        print(f"relic-distill: command channel failed: {exc}", file=sys.stderr)
        return None
    if done.returncode != 0:
        print(f"relic-distill: command exit {done.returncode}: {done.stderr[-400:]}", file=sys.stderr)
        return None
    return done.stdout


def via_http(prompt: str, timeout: float) -> str | None:
    base = os.environ.get("RELIC_DISTILL_BASE_URL", "").strip().rstrip("/")
    if not base:
        return None
    key = os.environ.get("RELIC_DISTILL_API_KEY", "")
    model = os.environ.get("RELIC_DISTILL_MODEL", "gpt-4o-mini")
    payload = json.dumps(
        {
            "model": model,
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0,
            "response_format": {"type": "json_object"},
        }
    ).encode()
    request = urllib.request.Request(
        f"{base}/chat/completions",
        data=payload,
        headers={"Content-Type": "application/json", "Authorization": f"Bearer {key}"},
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            body = json.load(response)
    except (urllib.error.URLError, OSError, ValueError) as exc:
        print(f"relic-distill: http channel failed: {exc}", file=sys.stderr)
        return None
    try:
        return body["choices"][0]["message"]["content"]
    except (KeyError, IndexError):
        return None


def extract_json(text: str | None) -> dict | None:
    if not text:
        return None
    text = text.strip()
    if text.startswith("```"):
        text = re.sub(r"^```[a-zA-Z]*\n?", "", text)
        text = re.sub(r"\n?```$", "", text.strip())
    start, end = text.find("{"), text.rfind("}")
    if start < 0 or end <= start:
        return None
    try:
        return json.loads(text[start : end + 1])
    except ValueError:
        return None


# ------------------------------------------------------------ fallback distiller

# Sentences that carry a preference, a prohibition, or a verified fact.
DURABLE_MARKERS = (
    "不要",
    "必须",
    "统一",
    "禁止",
    "改用",
    "记得",
    "do not",
    "don't",
    "must ",
    "always ",
    "never ",
    "instead of",
    "prefer ",
    "remember",
    "要求",
    "注意",
)
EVIDENCE_RE = re.compile(
    r"(`[^`]+`|[\w~./-]+\.(?:md|py|R|Rmd|csv|tsv|json|ya?ml|pdf|docx|pptx|xlsx|html|png|svg)\b"
    r"|/Volumes/|/Users/|/home/|c3g\.tun|c4g\.tun|dgx[0-9]*\.tun"
    r"|\b\d[\d,.]*\s*(?:Mb|kb|Gb|%|patients|samples|genes|reads|isolates)\b)"
)
TRIVIAL_TURNS = {"ok", "okay", "确认", "yes", "no", "继续", "好", "好的", "done", "1", "test"}


def find_vault(start: pathlib.Path | None = None) -> pathlib.Path | None:
    """Locate the vault the distiller is feeding, without extra configuration."""
    here = (start or pathlib.Path.cwd()).resolve()
    for candidate in [here, *here.parents]:
        if (candidate / ".relic").is_dir() and (candidate / "entries").is_dir():
            return candidate
    fallback = pathlib.Path.home() / "relic-vault"
    return fallback if (fallback / ".relic").is_dir() else None


def subject_vocabulary(vault: pathlib.Path, limit: int = 120) -> list[str]:
    """Subject tags already in use, most used first.

    Handing the model the vault's own vocabulary is what keeps tags from
    sprawling into hundreds of one-off words: the model is told to reuse, and
    only invent when nothing fits.
    """
    import collections

    counts: collections.Counter[str] = collections.Counter()
    for path in vault.rglob("*.md"):
        if ".relic" in path.parts or "sources" in path.parts or "reflections" in path.parts:
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except OSError:
            continue
        if not text.startswith("---\n"):
            continue
        front, _, _ = text[4:].partition("\n---\n")
        inside = False
        for line in front.splitlines():
            if line.startswith("tags:"):
                inside = True
                continue
            if inside and line.startswith("- "):
                tag = line[2:].strip()
                if tag and ":" not in tag:
                    counts[tag] += 1
            elif inside and line.strip():
                inside = False
    return [tag for tag, count in counts.most_common(limit) if count >= 2]


def project_slug(project: str) -> str:
    tail = project.rstrip("/").split("/")[-1] or "unknown"
    slug = re.sub(r"[^a-zA-Z0-9]+", "-", tail).strip("-").lower()
    return slug[:32] or "unknown"


def parse_turns(outcome: str) -> list[tuple[str, str]]:
    turns: list[tuple[str, str]] = []
    for chunk in re.split(r"\n##\s*轮次\s+\S+\s*\n", outcome):
        if not chunk.strip():
            continue
        user = re.split(r"\n最终回复：|\n### 最终回复\n", chunk, maxsplit=1)
        ask = user[0].replace("用户：", "").strip()
        reply = user[1].strip() if len(user) > 1 else ""
        turns.append((ask, reply))
    return turns


def compact(text: str) -> str:
    return re.sub(r"\s+", " ", text).strip()


def sentences(text: str) -> list[str]:
    parts = re.split(r"(?<=[。！？；])|(?<=\.)\s+|\n", text)
    return [p.strip(" -*•\t") for p in parts if p and p.strip()]


def fallback(request: dict) -> dict:
    """Extractive draft: keep what the session actually decided, drop the rest."""
    source = request.get("source") or {}
    data = source.get("input") or {}
    outcome = str(data.get("outcome", ""))
    turns = parse_turns(outcome)
    slug = project_slug(str(data.get("project", "")))

    asks = [ask for ask, _ in turns if ask.strip()]
    substantive = [a for a in asks if a.strip().lower() not in TRIVIAL_TURNS and len(a.strip()) > 12]
    if not substantive and len(outcome) < 400:
        return {
            "knowledge": {
                "title": f"{slug} · 历史会话（无持久内容）",
                "content": (data.get("title") or "empty session")[:400],
                "kind": "lesson",
                "confidence": 0.4,
                "tags": ["src:codex-history", f"project:{slug}", "pool:preconscious"],
            },
            "recommendation": "skip",
            "rationale": "会话没有实质轮次，未发现可复用的持久信息。",
            "evidence_indices": [],
            "related_entry_ids": [],
        }

    durable: list[str] = []
    for sentence in sentences(outcome):
        low = sentence.lower()
        if len(sentence) < 24 or len(sentence) > 400:
            continue
        if any(marker in low for marker in DURABLE_MARKERS) or EVIDENCE_RE.search(sentence):
            if sentence not in durable:
                durable.append(sentence)
        if len(durable) >= 6:
            break

    if not durable:
        durable = substantive[:2]

    evidence_hits = sum(1 for s in durable if EVIDENCE_RE.search(s))
    confidence = 0.6 if evidence_hits >= 2 else 0.5
    first = substantive[0] if substantive else (data.get("title") or "历史会话")
    headline = compact(first)[:48]
    title = f"{slug} · 历史会话：{headline}"
    content = "\n".join(f"- {s}" for s in durable)
    if asks:
        content = f"**原始诉求**：{compact(asks[0])[:300]}\n\n**可复用要点**\n{content}"

    return {
        "knowledge": {
            "title": title,
            "content": content[:4000],
            "kind": "lesson" if any("不要" in s or "do not" in s.lower() for s in durable) else "knowledge",
            "confidence": confidence,
            "tags": ["src:codex-history", f"project:{slug}", "pool:preconscious"],
        },
        "recommendation": "new",
        "rationale": f"启发式提炼（无模型通道）：从 {len(turns)} 个轮次中抽取 {len(durable)} 条带偏好/证据标记的句子。",
        "evidence_indices": [],
        "related_entry_ids": [],
    }


# ------------------------------------------------------------------- validation


def normalise(candidate: dict, request: dict, fallback_draft: dict) -> dict:
    """Coerce any model answer into the exact contract Relic parses."""
    knowledge = candidate.get("knowledge") if isinstance(candidate, dict) else None
    if not isinstance(knowledge, dict):
        return fallback_draft
    draft = fallback_draft["knowledge"]
    title = str(knowledge.get("title") or draft["title"]).strip()[:200]
    content = str(knowledge.get("content") or "").strip()
    if not content:
        return fallback_draft
    kind = str(knowledge.get("kind") or "knowledge").strip().lower()
    if kind not in KINDS:
        kind = "knowledge"
    try:
        confidence = float(knowledge.get("confidence", 0.5))
    except (TypeError, ValueError):
        confidence = 0.5
    tags = knowledge.get("tags") or []
    if not isinstance(tags, list):
        tags = []
    tags = [str(t).strip() for t in tags if str(t).strip()][:12]

    # Keep the vault's tag convention: scoping tags are namespaced, subjects are
    # bare. A model that omits the scope tags gets them added, never renamed away.
    for tag, value in (("src:codex-history", True), (f"project:{project_slug(str((request.get('source') or {}).get('input', {}).get('project', '')))}", True), ("pool:preconscious", True)):
        if tag not in tags and value:
            tags.append(tag)

    allowed_related = {item.get("id") for item in request.get("related_knowledge") or []}
    recommendation = str(candidate.get("recommendation") or "new").strip().lower()
    if recommendation not in ("new", "update", "skip"):
        recommendation = "new"
    related = [r for r in (candidate.get("related_entry_ids") or []) if r in allowed_related]
    if recommendation == "update" and not related:
        recommendation = "new"
    evidence_count = len(((request.get("source") or {}).get("input") or {}).get("evidence") or [])
    indices = [i for i in (candidate.get("evidence_indices") or []) if isinstance(i, int) and 0 <= i < evidence_count]

    return {
        "knowledge": {
            "title": title or draft["title"],
            "content": content,
            "kind": kind,
            "confidence": max(0.0, min(1.0, confidence)),
            "tags": tags,
        },
        "recommendation": recommendation,
        "rationale": str(candidate.get("rationale") or "")[:1000] or "模型未给出理由。",
        "evidence_indices": indices,
        "related_entry_ids": related,
    }


def main() -> int:
    raw = sys.stdin.read()
    try:
        request = json.loads(raw)
    except ValueError:
        print("relic-distill: request is not JSON", file=sys.stderr)
        return 1

    source = request.get("source") or {}
    data = source.get("input") or {}
    related = request.get("related_knowledge") or []
    vault = find_vault()
    vocabulary = subject_vocabulary(vault) if vault else []
    prompt = PROMPT_TEMPLATE.format(
        instructions=request.get("instructions", ""),
        event_id=data.get("event_id", ""),
        project=data.get("project", ""),
        source_agent=data.get("source_agent", ""),
        title=data.get("title", ""),
        context=data.get("context", ""),
        action=data.get("action", ""),
        outcome=data.get("outcome", ""),
        evidence="\n".join(f"[{i}] {e}" for i, e in enumerate(data.get("evidence") or [])) or "(none)",
        related="\n\n".join(f"[{item.get('id')}] {item.get('title')}\n{item.get('content')}" for item in related) or "(none)",
        vocabulary=", ".join(vocabulary) or "(vault is empty)",
    )

    fallback_draft = fallback(request)
    timeout = float(os.environ.get("RELIC_DISTILL_TIMEOUT", "45"))
    answer = extract_json(via_command(prompt, timeout)) or extract_json(via_http(prompt, timeout))
    if os.environ.get("RELIC_DISTILL_DEBUG"):
        print(f"relic-distill: model answer = {answer}", file=sys.stderr)

    result = normalise(answer, request, fallback_draft) if answer else fallback_draft
    json.dump(result, sys.stdout, ensure_ascii=False)
    return 0


if __name__ == "__main__":
    sys.exit(main())
