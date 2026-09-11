#!/usr/bin/env python3
"""relic-relate — add the relations the vault never recorded.

The vault's 182 imported bullets carry dangling pseudo-links to Codex's own
memory groups; the curated entries carry almost none. This script writes the
genuine edges an author would draw by hand, and supersedes the one entry that a
later entry actually replaced.

Every edge below cites why it exists. Run with --dry-run to inspect first.

Usage:
  python3 tools/relic-relate.py --vault ~/relic-vault --relic ./target/debug/relic --dry-run
"""

from __future__ import annotations

import argparse
import pathlib
import subprocess
import sys

# id -> (links to add, reason)
RELATIONS: dict[str, tuple[list[str], str]] = {
    # --- 图谱 / UI 投影层：同一份推导的四个读法 -----------------------------
    "relic-20260910-783cb1": (
        ["relic-20260906-7bb5d4", "relic-20260910-a5a654", "relic-20260910-b2998a", "relic-20260911-011b4e"],
        "topic sphere 是 UI 内嵌资产；它渲染的正是 011b4e 描述的图谱，几何验证方法由 b2998a 记录",
    ),
    "relic-20260910-a5a654": (
        ["relic-20260906-7bb5d4", "relic-20260910-b2998a", "relic-20260901-b4f78e"],
        "分布视图与 brain 是同一份数据的两面；两者都只显示 effective confidence（b4f78e 的语义）",
    ),
    "relic-20260910-b2998a": (
        ["relic-20260910-783cb1"],
        "同一套 headless-Chrome/CDP 验证方法，几何校验而非截图",
    ),
    "relic-20260911-011b4e": (
        ["relic-20260911-021e17", "relic-20260910-783cb1"],
        "矢量层+逻辑层架构与其已知的三个推导陷阱是同一设计的两侧",
    ),
    "relic-20260911-021e17": (
        ["relic-20260911-011b4e"],
        "陷阱条目描述的就是 011b4e 架构中关系推导部分的失败模式",
    ),
    # --- 捕获 → 提炼 → 导入 管线 ------------------------------------------
    "relic-20260910-ee6fdf": (
        ["relic-capture-26a63e5a-64cb-4c63-806f-a7f2badf8634", "relic-20260906-8b9a7c", "relic-20260907-9f1813"],
        "codex-memory 导入依赖蒸馏后端契约（26a63e5a）与捕获阶段 1 的持久性契约（8b9a7c），并落实 9f1813 的产品方向",
    ),
    "relic-capture-26a63e5a-64cb-4c63-806f-a7f2badf8634": (
        ["relic-20260906-8b9a7c"],
        "命令蒸馏的排队/恢复语义建立在捕获阶段 1 的持久性之上",
    ),
    "relic-20260906-8b9a7c": (
        ["relic-20260902-4cb81f"],
        "capture 阶段 1 是 1.0 配置/watch 里程碑之后的下一步",
    ),
    # --- 宿主集成 ---------------------------------------------------------
    "relic-20260906-718757": (
        ["relic-20260901-7c5ee0"],
        "两者都是宿主接线约束：MCP 启动命令必须是可执行文件；DSH 只读特定位置的 AGENTS.md",
    ),
    "relic-20260901-7c5ee0": (
        ["relic-20260906-718757", "relic-20260906-7bb5d4"],
        "同属宿主集成层；7bb5d4 记录了 UI 侧的接线约束",
    ),
    # --- 版本里程碑链 -----------------------------------------------------
    "relic-20260902-ef49cd": (["relic-20260902-f0215b"], "v0.3 → v0.4 里程碑链"),
    "relic-20260902-f0215b": (["relic-20260902-12b7ae"], "v0.4 → v0.5 里程碑链"),
    "relic-20260902-12b7ae": (["relic-20260902-4cb81f"], "v0.5 → v1.0 里程碑链"),
    "relic-20260901-b4f78e": (["relic-20260910-a5a654"], "置信度衰减语义是该分布视图显示 effective confidence 的依据"),
    "relic-20260907-2924e8": (["relic-20260907-9f1813"], "竞品评估用于确定 9f1813 的产品定位"),
}

# old_id -> new_id: 新条目实现了旧条目明确留作 future work 的部分
SUPERSEDES: list[tuple[str, str, str]] = [
    (
        "relic-20260901-00a43b",
        "relic-20260902-ef49cd",
        "旧条目写明「SSE, sessions, and OAuth remain future work」，次日条目正是实现了这三项",
    ),
]


def read_links(path: pathlib.Path) -> list[str]:
    text = path.read_text(encoding="utf-8")
    lines = text.splitlines()
    links: list[str] = []
    inside = False
    for line in lines:
        if line.startswith("links:"):
            inside = True
            continue
        if inside:
            if line.startswith("- "):
                links.append(line[2:].strip())
            elif line.strip():
                break
    return links


def find_entry(vault: pathlib.Path, entry_id: str) -> pathlib.Path | None:
    for path in vault.rglob("*.md"):
        if ".relic" in path.parts or "sources" in path.parts:
            continue
        head = path.read_text(encoding="utf-8")[:400]
        if f"id: {entry_id}\n" in head:
            return path
    return None


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--vault", default=str(pathlib.Path.home() / "relic-vault"))
    parser.add_argument("--relic", default="./target/debug/relic")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    vault = pathlib.Path(args.vault).expanduser()
    # The CLI runs with cwd=vault, so a relative path would resolve there.
    relic = str(pathlib.Path(args.relic).resolve())
    if not pathlib.Path(relic).exists():
        print(f"relic binary not found: {relic}")
        return 1
    planned = 0
    for entry_id, (targets, reason) in RELATIONS.items():
        path = find_entry(vault, entry_id)
        if path is None:
            print(f"MISSING entry {entry_id}")
            return 1
        existing = read_links(path)
        merged = existing + [t for t in targets if t not in existing and t != entry_id]
        for target in merged:
            if target not in existing:
                if find_entry(vault, target) is None and not target.startswith("sources/"):
                    print(f"WARNING: {entry_id} -> {target} does not resolve to an entry")
        if merged == existing:
            print(f"  = {entry_id}: already linked")
            continue
        planned += len(merged) - len(existing)
        print(f"  + {entry_id} -> {[t for t in merged if t not in existing]}")
        print(f"      why: {reason}")
        if not args.dry_run:
            done = subprocess.run(
                [relic, "update", entry_id, "--links", ",".join(merged)],
                cwd=vault,
                capture_output=True,
                text=True,
            )
            if done.returncode != 0:
                print(f"    FAILED: {done.stderr.strip()}")
                return 1

    for old_id, new_id, reason in SUPERSEDES:
        print(f"  > supersede {old_id} by {new_id}\n      why: {reason}")
        if not args.dry_run:
            done = subprocess.run(
                [relic, "supersede", old_id, new_id],
                cwd=vault,
                capture_output=True,
                text=True,
            )
            if done.returncode != 0:
                print(f"    FAILED: {done.stderr.strip()}")
                return 1

    print(f"\nplanned link additions: {planned}; supersede pairs: {len(SUPERSEDES)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
