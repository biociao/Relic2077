#!/usr/bin/env python3
"""relic-tidy — reorganize a Relic vault's imported bulk memories.

Idempotent. Dry-run by default; pass --apply to write.

Design (see docs/ for the rationale):
  * namespaced tags are *scoping* tags, never subject tags:
      src:<agent>      provenance of the material
      project:<slug>   which project the memory belongs to
      section:<kind>   pref | failure | fact  (Codex memory section)
      pool:preconscious  not promoted to the main memory
  * subject tags stay bare words (ui, rust, mcp, verification, ...)
  * bulk entries get a scannable title: "<project> · <章节>：<断言片段>"
    while the original task group / section survive in the body header
  * confidence becomes discriminating: evidence-backed vs assertion-only
  * the 182 host-imported bullets move to the preconscious pool (status=fading)

Usage:
  python3 tools/relic-tidy.py [--vault ~/relic-vault] [--apply] [--json report.json]
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import shutil
import sys
import tempfile
from pathlib import Path

import yaml

# ---------------------------------------------------------------- yaml plumbing

class RawLoader(yaml.SafeLoader):
    """SafeLoader that keeps timestamps as plain strings."""


RawLoader.yaml_implicit_resolvers = {
    k: [(tag, regexp) for tag, regexp in v if tag != "tag:yaml.org,2002:timestamp"]
    for k, v in yaml.SafeLoader.yaml_implicit_resolvers.items()
}


class RawDumper(yaml.SafeDumper):
    """SafeDumper that emits timestamp-looking strings unquoted, like serde_yaml."""


RawDumper.yaml_implicit_resolvers = {
    k: [(tag, regexp) for tag, regexp in v if tag != "tag:yaml.org,2002:timestamp"]
    for k, v in yaml.SafeDumper.yaml_implicit_resolvers.items()
}


def load_front_matter(text: str):
    if not text.startswith("---\n"):
        raise ValueError("missing front matter")
    yaml_text, _, body = text[4:].partition("\n---\n")
    meta = yaml.load(yaml_text, Loader=RawLoader)
    return meta, body


def render(meta: dict, body: str) -> str:
    """Mirror Relic's Entry::render byte shape."""
    out = yaml.dump(
        meta,
        Dumper=RawDumper,
        default_flow_style=False,
        allow_unicode=True,
        sort_keys=False,
        width=10_000,
    ).strip()
    return f"---\n{out}\n---\n\n{body.strip()}\n"


def atomic_write(path: Path, text: str) -> None:
    fd, tmp = tempfile.mkstemp(dir=str(path.parent), prefix=".tidy-", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(text)
        shutil.copystat(path, tmp)
        os.replace(tmp, path)
    except BaseException:
        os.unlink(tmp)
        raise


# ------------------------------------------------------------- classification

TASK_GROUP_SLUG = {
    "GI02_JITC_rebuttal": "gi02-jitc",
    "pumch-isowast": "isowast",
    "Pan.C.par": "pan-c-par",
    "research-harness-benchmark": "rhb",
    "GI03": "gi03",
    "hwb": "hwb",
    "Scientific/manuscript deliverables and presentations outside active repositories": "manuscript",
    "PMAID regulatory infection-AI presentation": "pmaid-ppt",
    "Remote_DSH_Center": "remote-dsh",
    "academic CV and conference speaker bio refinement": "cv-bio",
    "dgx21.tun": "dgx21",
    "BGI/GS": "bgi-gs",
    "Genpilot": "genpilot",
    "vaginal live biotherapeutic": "ctv05",
    "Rosalind/workbench life-sciences workflows and plugin-gated analysis": "rosalind",
    "one-page Aedes atlas": "aedes-atlas",
    "professional technology workflow-image editing": "workflow-image",
}

SECTION_CN = {
    "Failures and how to do differently": "教训",
    "User preferences": "偏好",
    "Reusable knowledge": "知识",
}
SECTION_SLUG = {
    "Failures and how to do differently": "failure",
    "User preferences": "pref",
    "Reusable knowledge": "fact",
}

EVIDENCE_RE = re.compile(
    r"("
    r"`[^`]+`"                                                    # any code span
    r"|[\w~./-]+\.(?:md|py|R|Rmd|csv|tsv|json|ya?ml|pdf|docx|pptx|xlsx|html|h5ad|png|svg|log)\b"
    r"|/Volumes/|/Users/|/home/|c4g\.tun|dgx[0-9]*\.tun|CHAOSBJ0|CHAOSDVC"
    r"|\b\d[\d,.]*\s*(?:Mb|kb|Gb|bp|reads|samples|genes|isolates|patients|%|hours|minutes|seconds)\b"
    r"|\b[0-9a-f]{7,40}\b"                                        # commit hash
    r")"
)

# ------------------------------------------------------------- transformations


def split_title(title: str):
    parts = [p.strip() for p in title.split(" · ")]
    if len(parts) >= 3:
        return parts[0], parts[-2], parts[-1]
    if len(parts) == 2:
        return parts[0], parts[1], ""
    return parts[0], "", ""


def slug_for_group(group: str) -> str:
    for prefix, slug in TASK_GROUP_SLUG.items():
        if group.startswith(prefix):
            return slug
    return re.sub(r"[^a-z0-9]+", "-", group.lower()).strip("-")[:24] or "misc"


def snippet(text: str, limit: int = 64) -> str:
    """First sentence/clause of the bullet, trimmed to `limit` characters."""
    cleaned = re.sub(r"\s+", " ", text).strip()
    cleaned = cleaned.lstrip("*-• ").strip()
    cleaned = cleaned.split(" -> ")[0]
    for stop in ("。", ". ", "; ", "；"):
        head = cleaned.split(stop)[0]
        if 12 <= len(head) <= limit:
            cleaned = head
            break
    if len(cleaned) > limit:
        cleaned = cleaned[:limit].rstrip(" ,;:，；：") + "…"
    return cleaned


def tidy_bulk(meta: dict, body: str, now: str):
    """Return (new_meta, new_body, notes) for one host-imported bullet."""
    notes = []
    group, section, _tail = split_title(meta.get("title", ""))
    slug = slug_for_group(group)
    section_cn = SECTION_CN.get(section, "知识")
    section_slug = SECTION_SLUG.get(section, "fact")

    core = "\n".join(
        line
        for line in body.strip().splitlines()
        if line.strip() and not line.startswith("**Scope**") and not line.startswith("**Applies to**")
    )
    assertion = snippet(core) or snippet(meta.get("title", ""))

    tags = ["src:codex-memory", f"project:{slug}", f"section:{section_slug}", "pool:preconscious"]
    for tag in meta.get("tags", []):
        if tag in ("codex-memory", "unverified"):
            continue
        if tag not in tags:
            tags.append(tag)

    links = [l for l in meta.get("links", []) if not l.startswith("codex-memory:codex:")]
    if len(links) != len(meta.get("links", [])):
        notes.append("dropped dangling codex-memory pseudo-link")

    confidence = 0.65 if EVIDENCE_RE.search(core) else 0.5

    header = (
        f"> 来自 Codex 记忆导入 · 任务组：{group or '未标注'} · 章节：{section or '未标注'}\n"
    )
    if header.strip() not in body:
        new_body = header + "\n" + body.strip()
    else:
        new_body = body.strip()

    new_meta = dict(meta)
    new_meta["title"] = f"{slug} · {section_cn}：{assertion}"
    new_meta["status"] = "fading"
    new_meta["confidence"] = confidence
    new_meta["tags"] = tags
    new_meta["links"] = links
    new_meta["updated"] = now
    return new_meta, new_body, notes


def tidy_curated(meta: dict, body: str, now: str):
    """Curated entries keep their status/title; only the tag convention applies."""
    notes = []
    before = list(meta.get("tags", []))
    tags = []
    for tag in before:
        mapped = {"relic2077": "relic", "codex-memory": "codex-memory-import"}.get(tag, tag)
        if tag == "unverified":
            notes.append("dropped unverifiable marker tag")
            continue
        if mapped != tag:
            notes.append(f"tag {tag} -> {mapped}")
        if mapped not in tags:
            tags.append(mapped)
    meta = dict(meta)
    meta["tags"] = tags
    if tags != before:
        meta["updated"] = now
    return meta, body, notes


def tidy_placeholder(meta: dict, body: str, now: str):
    meta = dict(meta)
    meta["status"] = "archived"
    if "placeholder" not in meta.get("tags", []):
        meta["tags"] = ["placeholder"]
    meta["updated"] = now
    return meta, body, ["archived empty placeholder entry"]


# ---------------------------------------------------------------------- main


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--vault", default=os.path.expanduser("~/relic-vault"))
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--json")
    args = parser.parse_args()

    vault = Path(args.vault).expanduser()
    now = dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ")
    report = {"vault": str(vault), "applied": args.apply, "changed": [], "skipped": [], "errors": []}

    for path in sorted(vault.rglob("*.md")):
        if ".relic" in path.parts or "sources" in path.parts:
            continue
        text = path.read_text(encoding="utf-8")
        if not text.startswith("---\n"):
            report["skipped"].append({"path": str(path), "why": "no front matter"})
            continue
        try:
            meta, body = load_front_matter(text)
        except Exception as exc:  # noqa: BLE001
            report["errors"].append({"path": str(path), "why": str(exc)})
            continue
        if not meta.get("id"):
            report["skipped"].append({"path": str(path), "why": "not an entry"})
            continue

        rel = path.relative_to(vault).as_posix()
        notes: list[str] = []
        if "codex-memory" in meta.get("source_agents", []):
            new_meta, new_body, notes = tidy_bulk(meta, body, now)
        elif len(body.strip()) < 120 and not meta.get("tags"):
            new_meta, new_body, notes = tidy_placeholder(meta, body, now)
        else:
            new_meta, new_body, notes = tidy_curated(meta, body, now)

        rendered = render(new_meta, new_body)
        if rendered == text:
            report["skipped"].append({"path": rel, "why": "already tidy"})
            continue
        # A rewrite that changes nothing but YAML formatting is churn: leave the
        # file's bytes alone so `updated` and the diff stay meaningful.
        semantic_before = {k: v for k, v in meta.items() if k != "updated"}
        semantic_after = {k: v for k, v in new_meta.items() if k != "updated"}
        if semantic_before == semantic_after and new_body.strip() == body.strip():
            report["skipped"].append({"path": rel, "why": "formatting only"})
            continue
        # round-trip guard: the rewritten file must parse back to exactly the
        # metadata we intended, or we refuse to touch it.
        back_meta, back_body = load_front_matter(rendered)
        if back_meta != new_meta or back_body.strip() != new_body.strip():
            report["errors"].append({"path": rel, "why": "YAML round-trip mismatch"})
            continue

        entry = {
            "path": rel,
            "id": new_meta["id"],
            "title_before": meta.get("title"),
            "title_after": new_meta["title"],
            "tags_before": meta.get("tags", []),
            "tags_after": new_meta["tags"],
            "status": new_meta["status"],
            "confidence": new_meta["confidence"],
            "notes": notes,
        }
        report["changed"].append(entry)
        if args.apply:
            atomic_write(path, rendered)

    changed = report["changed"]
    out = {
        "changed": len(changed),
        "skipped": len(report["skipped"]),
        "errors": len(report["errors"]),
        "by_project": {},
        "by_status": {},
        "by_confidence": {},
    }
    for entry in changed:
        project = next((t for t in entry["tags_after"] if t.startswith("project:")), "-")
        out["by_project"][project] = out["by_project"].get(project, 0) + 1
        out["by_status"][entry["status"]] = out["by_status"].get(entry["status"], 0) + 1
        out["by_confidence"][str(entry["confidence"])] = (
            out["by_confidence"].get(str(entry["confidence"]), 0) + 1
        )
    print(json.dumps(out, ensure_ascii=False, indent=2))
    if report["errors"]:
        print("ERRORS:", json.dumps(report["errors"], ensure_ascii=False, indent=2))
    print("\nsample rewrites:")
    for entry in changed[:6]:
        print(f"  {entry['path']}")
        print(f"    - {entry['title_before'][:96]}")
        print(f"    + {entry['title_after']}")
        print(f"      tags={entry['tags_after']} status={entry['status']} conf={entry['confidence']}")
    if args.json:
        Path(args.json).write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
