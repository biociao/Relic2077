#!/usr/bin/env python3
"""relic-triage — review the distilled queue drafts against a stated policy.

The reviewer here is a policy, not a person, so the policy has to be written
down and has to be conservative:

  reject  the distiller itself recommended `skip`, or the draft has no usable
          title/content (thin drafts are noise in a vault that already had a
          noise problem)
  accept  everything else, verbatim, and then demote the new entry into the
          preconscious pool (`status: fading` + `pool:preconscious`) because a
          machine-distilled, unverified memory is exactly what that pool is for

Accepting is reversible: entries keep their id and provenance, and promotion to
the main memory is `relic update <id> --status active`.

Usage:
  python3 tools/relic-triage.py --vault ~/relic-vault --relic ~/.cargo/bin/relic --dry-run
"""

from __future__ import annotations

import argparse
import json
import pathlib
import subprocess
import sys
import tempfile

MIN_CONTENT = 200


def decide(item: dict) -> tuple[str, str]:
    candidate = item.get("candidate") or {}
    knowledge = candidate.get("knowledge") or {}
    trace = candidate.get("distillation") or {}
    recommendation = trace.get("recommendation") or "new"
    title = (knowledge.get("title") or "").strip()
    content = (knowledge.get("content") or "").strip()

    if recommendation == "skip":
        return "reject", f"提炼器判定为不可持久：{trace.get('rationale') or '未给理由'}"
    if not title or len(content) < MIN_CONTENT:
        return "reject", f"草稿过薄（title={len(title)} 字符，content={len(content)} 字符），不足以成为一条记忆"
    return "accept", trace.get("rationale") or "按政策接受提炼草稿"


def active_preconscious(vault: pathlib.Path) -> list[dict]:
    """Active entries that still declare the preconscious pool."""
    import yaml

    class Loader(yaml.SafeLoader):
        pass

    Loader.yaml_implicit_resolvers = {
        k: [(tag, rx) for tag, rx in v if tag != "tag:yaml.org,2002:timestamp"]
        for k, v in yaml.SafeLoader.yaml_implicit_resolvers.items()
    }
    found = []
    for path in sorted(vault.rglob("*.md")):
        if ".relic" in path.parts or "sources" in path.parts:
            continue
        text = path.read_text(encoding="utf-8")
        if not text.startswith("---\n"):
            continue
        yaml_text, _, _ = text[4:].partition("\n---\n")
        meta = yaml.load(yaml_text, Loader=Loader)
        if not meta or not meta.get("id"):
            continue
        if meta.get("status") == "active" and "pool:preconscious" in (meta.get("tags") or []):
            found.append(meta)
    return found


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--vault", default=str(pathlib.Path.home() / "relic-vault"))
    parser.add_argument("--relic", default="relic")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--limit", type=int, default=1000)
    args = parser.parse_args()

    vault = pathlib.Path(args.vault).expanduser()
    listing = subprocess.run(
        [args.relic, "queue", "list"], cwd=vault, capture_output=True, text=True, check=True
    )
    items = json.loads(listing.stdout)
    pending = [i for i in items if i["status"] == "needs_review"]
    print(f"queue: {len(items)} items, {len(pending)} awaiting review")

    accepted = rejected = failed = 0
    reasons: dict[str, int] = {}
    for item in pending[: args.limit]:
        decision, reason = decide(item)
        reasons[f"{decision}: {reason[:40]}"] = reasons.get(f"{decision}: {reason[:40]}", 0) + 1
        if decision == "accept":
            accepted += 1
        else:
            rejected += 1
        if args.dry_run:
            continue
        payload = {"decision": decision, "reason": reason}
        if decision == "accept":
            payload["knowledge"] = item["candidate"]["knowledge"]
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False, encoding="utf-8") as handle:
            json.dump(payload, handle, ensure_ascii=False)
            path = handle.name
        done = subprocess.run(
            [args.relic, "queue", "review", item["id"], path],
            cwd=vault,
            capture_output=True,
            text=True,
        )
        pathlib.Path(path).unlink(missing_ok=True)
        if done.returncode != 0:
            failed += 1
            print(f"  review failed for {item['id']}: {done.stderr.strip()[:200]}")

    print(f"\naccepted={accepted} rejected={rejected} failed={failed}")
    print("decision mix (top):")
    for reason, count in sorted(reasons.items(), key=lambda kv: -kv[1])[:6]:
        print(f"  {count:4d}  {reason}")

    # Accepted drafts were created as `active`; a machine-distilled, unverified
    # memory belongs in the preconscious pool until someone promotes it.
    demoted = 0
    if not args.dry_run:
        for meta in active_preconscious(vault):
            done = subprocess.run(
                [args.relic, "update", meta["id"], "--status", "fading"],
                cwd=vault,
                capture_output=True,
                text=True,
            )
            if done.returncode == 0:
                demoted += 1
        print(f"demoted into pool:preconscious: {demoted}")
        subprocess.run([args.relic, "reindex"], cwd=vault, check=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
