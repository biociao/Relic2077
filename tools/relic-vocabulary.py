#!/usr/bin/env python3
"""relic-vocabulary — keep subject tags a vocabulary, not a free-for-all.

A memory system is only as searchable as its tag vocabulary is shared. The
distiller is good at naming what a memory is about, but left alone it invents a
fresh label per memory: the first pass over 149 distilled entries produced 673
new one-off subject tags, which is the same disease as the old single shared
`codex-memory` tag, just inverted.

Rule, in order:
  1. project names used as subjects become scopes (`hwb` → `project:hwb`)
  2. case variants collapse to the most used spelling (`treg` → `Treg`)
  3. known synonyms collapse (`benchmark-evaluation` → `benchmark`)
  4. a subject survives only if the vault uses it at least `--min-uses` times,
     or it is in the curated vocabulary in `.relic/taxonomy.md`
     — **but only for machine-generated entries** (`src:codex*`). A hand-written
     memory's tags are deliberate: `analysis`, `canvas`, `contradiction` stay.
  5. scope tags (`src:`/`project:`/`section:`/`pool:`) are never touched

Nothing is lost from the body, so full-text search still finds a dropped word;
what changes is that the *subject* layer stays a layer.

Usage:
  python3 tools/relic-vocabulary.py --vault ~/relic-vault --dry-run
  python3 tools/relic-vocabulary.py --vault ~/relic-vault --apply
"""

from __future__ import annotations

import argparse
import collections
import pathlib
import subprocess
import sys

import yaml

# subject tag -> canonical replacement ("" means: drop it, the scope carries it)
SYNONYMS = {
    "benchmark-evaluation": "benchmark",
    "evidence-levels": "evidence-grading",
    "relic2077": "relic",
    "docx-revision": "docx",
    "manuscript-revision": "scientific-writing",
}

# project names that belong in the scope namespace, not the subject namespace
PROJECT_SUBJECTS = {
    "hwb": "hwb",
    "pumch-isowast": "isowast",
    "gi02-jitc": "gi02-jitc",
    "pan-c-par": "pan-c-par",
    "pmaid": "pmaid-mc",
    "renji-mngs": "renji-mngs",
}

# subjects that stay even at a single use: the curated vocabulary
CURATED = {
    "relic", "ui", "rust", "mcp", "api", "3d", "canvas", "stats", "svg",
    "transport", "sse", "sessions", "oauth", "security", "http", "git", "sync",
    "conflict", "config", "schema", "daemon", "watch", "confidence", "decay",
    "cli", "capture", "distillation", "queue", "durability", "recovery",
    "architecture", "design", "verification", "headless-chrome", "knowledge-graph",
    "vector-search", "rrf", "integration", "agents", "troubleshooting", "codex",
    "dsh", "competitive-research", "product-direction", "cognitive-memory",
    "recall-gating", "lesson", "placeholder",
}


class Loader(yaml.SafeLoader):
    pass


Loader.yaml_implicit_resolvers = {
    k: [(tag, rx) for tag, rx in v if tag != "tag:yaml.org,2002:timestamp"]
    for k, v in yaml.SafeLoader.yaml_implicit_resolvers.items()
}


class Dumper(yaml.SafeDumper):
    pass


Dumper.yaml_implicit_resolvers = {
    k: [(tag, rx) for tag, rx in v if tag != "tag:yaml.org,2002:timestamp"]
    for k, v in yaml.SafeDumper.yaml_implicit_resolvers.items()
}


def load(path: pathlib.Path):
    text = path.read_text(encoding="utf-8")
    if not text.startswith("---\n"):
        return None, None
    front, _, body = text[4:].partition("\n---\n")
    meta = yaml.load(front, Loader=Loader)
    if not meta or not meta.get("id"):
        return None, None
    return meta, body


def render(meta: dict, body: str) -> str:
    out = yaml.dump(meta, Dumper=Dumper, default_flow_style=False, allow_unicode=True,
                    sort_keys=False, width=10_000).strip()
    return f"---\n{out}\n---\n\n{body.strip()}\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--vault", default=str(pathlib.Path.home() / "relic-vault"))
    parser.add_argument("--relic", default="relic")
    parser.add_argument("--min-uses", type=int, default=2)
    parser.add_argument("--max-subjects", type=int, default=6)
    parser.add_argument("--apply", action="store_true")
    parser.add_argument(
        "--all",
        action="store_true",
        help="prune one-off subjects everywhere, including hand-written entries",
    )
    args = parser.parse_args()

    vault = pathlib.Path(args.vault).expanduser()
    entries = []
    for path in sorted(vault.rglob("*.md")):
        if ".relic" in path.parts or "sources" in path.parts or "reflections" in path.parts:
            continue
        meta, body = load(path)
        if meta:
            entries.append((path, meta, body))

    counts: collections.Counter[str] = collections.Counter()
    for _, meta, _ in entries:
        for tag in meta.get("tags", []):
            if ":" not in tag:
                counts[tag] += 1

    # case folding: the most used spelling wins, ties broken by first appearance
    spelling: dict[str, str] = {}
    for tag, _ in counts.most_common():
        spelling.setdefault(tag.lower(), tag)

    def canon(tag: str) -> str | None:
        if ":" in tag:
            return tag
        folded = spelling.get(tag.lower(), tag)
        folded = SYNONYMS.get(folded, folded)
        return folded or None

    canonical: collections.Counter[str] = collections.Counter()
    for _, meta, _ in entries:
        for tag in meta.get("tags", []):
            if ":" in tag:
                continue
            name = canon(tag)
            if name:
                canonical[name] += 1

    def keep(tag: str) -> bool:
        return tag in CURATED or canonical.get(tag, 0) >= args.min_uses

    def is_machine(meta: dict) -> bool:
        """Machine material gets the vocabulary rule; hand-written tags are kept."""
        return any(t.startswith("src:codex") for t in meta.get("tags", []))

    changed = 0
    dropped: collections.Counter[str] = collections.Counter()
    moved: collections.Counter[str] = collections.Counter()
    for path, meta, body in entries:
        before = list(meta.get("tags", []))
        subjects: list[str] = []
        scopes: list[str] = [t for t in before if ":" in t]
        for tag in before:
            if ":" in tag:
                continue
            name = canon(tag)
            if name is None:
                continue
            if name in PROJECT_SUBJECTS:
                scope = f"project:{PROJECT_SUBJECTS[name]}"
                if scope not in scopes:
                    scopes.append(scope)
                moved[name] += 1
                continue
            if not keep(name) and (args.all or is_machine(meta)):
                dropped[name] += 1
                continue
            if name not in subjects:
                subjects.append(name)
        subjects = sorted(subjects, key=lambda t: (-canonical.get(t, 0), t))[: args.max_subjects]
        after = scopes + subjects
        if after == before:
            continue
        changed += 1
        if args.apply:
            meta = dict(meta)
            meta["tags"] = after
            rendered = render(meta, body)
            back, _ = load_text(rendered)
            if back != meta:
                print(f"  refused (YAML round-trip mismatch): {path.name}")
                changed -= 1
                continue
            path.write_text(rendered, encoding="utf-8")

    print(f"entries: {len(entries)} | rewritten: {changed}")
    print(f"distinct subjects before: {len(counts)} | after: {sum(1 for t in canonical if keep(t))}")
    print(f"one-off subjects dropped: {sum(dropped.values())} uses across {len(dropped)} words")
    print("  most dropped:", dropped.most_common(8))
    print(f"project names moved into scopes: {sum(moved.values())} uses", dict(moved))
    if args.apply:
        subprocess.run([args.relic, "reindex"], cwd=vault, check=True)
    return 0


def load_text(text: str):
    front, _, body = text[4:].partition("\n---\n")
    return yaml.load(front, Loader=Loader), body


if __name__ == "__main__":
    sys.exit(main())
