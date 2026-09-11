<div align="center">

# Relic2077

### Retrieval-Enhanced Local Intelligence Cache

**English** | [简体中文](README.zh-CN.md)

![Relic2077 Agent Memory Core](assets/relic2077-agent-memory-core.png)

>Secure Your Soul

</div>

---

Relic2077 is a local-first, Git-native intelligence hub that preserves 

a single continuity of memory as AI agent tools rapidly evolve 

and compete to become the user's primary interface. 

It gives Codex, Claude, Cursor, DeepSeek Harness (DSH), and other MCP-compatible agents 

a unified memory that is fully owned by the user,

without locking personal knowledge to any model or vendor.

Markdown is the source of truth; SQLite is only a disposable search index. 

Your knowledge remains readable, portable, rebuildable, and yours.

</div>

## Current milestone

This repository contains the CLI, MCP integration, and a local browser UI:

- initialize a portable vault with a documented schema;
- create, read, list, and full-text search knowledge entries;
- organize knowledge, patterns, decisions, sources, attachments, and reflections;
- generate daily, weekly, or monthly reflection drafts;
- rebuild the entire SQLite FTS5 index from Markdown at any time;
- validate vault health with `relic doctor`.
- build a knowledge graph over your memories — typed, evidence-carrying
  relations plus a local vector space, both derived from Markdown and both
  disposable;
- expose the vault to Codex, Claude, Cursor, and other agents through a local
  STDIO MCP server;
- inspect memories, edit entries and configuration, and check vault health in
  a browser.

Git synchronization, remote MCP transport, and specialized agent adapters remain
future milestones. The storage format is already compatible with them.


## Install

```bash
cargo install --path .
```

This installs a single command named `relic`.

## Quick start

```bash
relic init ~/relic-vault
cd ~/relic-vault

relic add "RAG chunking strategy" \
  --content "Use 512–1024 token chunks for prose; validate against the corpus." \
  --tags rag,chunking \
  --confidence 0.82 \
  --source-agent codex

relic search "chunking"
relic update <entry-id> --confidence 0.9 --tags rag,verified
relic supersede <old-entry-id> <new-entry-id>
relic list --status active --tags rag --min-confidence 0.5
relic analyze
relic reflect --period weekly --auto --min-entries 5
relic graph build
relic graph stats
relic graph neighbors <entry-id> --depth 2
relic graph path <entry-id> <entry-id>
relic search "chunking" --mode hybrid
relic watch --once
relic stats
relic doctor
relic sync
relic import-codex-memory --dry-run
```

`relic list` displays effective confidence. Relic continuously decays the stored
confidence from `last_verified` using `confidence × e^(-decay_rate × years)`;
expired entries have an effective confidence of zero. Updating an entry's
confidence also refreshes `last_verified`. List filters include `--kind`,
`--status`, `--tags`, `--source-agent`, `--min-confidence`, and
`--max-confidence`.

## Open the local UI

Start the UI with an installed binary or directly from this repository:

```bash
relic ui --vault /path/to/relic-vault
# From the repository:
cargo run -- ui --vault /path/to/relic-vault
```

Open [http://127.0.0.1:7338](http://127.0.0.1:7338) in a modern browser. The UI
uses a local Rust server and browser assets embedded in the binary, providing
the same interface on Windows, macOS, and Linux without Node.js, a CDN, or a
separate frontend runtime. Rebuild the binary after editing files in `ui/`.

Because the assets are embedded at compile time, a running server can only show
the UI from the binary that started it. After changing anything under `ui/`,
rebuild **and restart** it, and make sure you restart the binary you actually
rebuilt — `cargo build` writes `target/debug/relic`, which does not update an
already installed `relic` on `PATH`:

```bash
cargo build            # or: cargo build --release
./target/debug/relic ui --vault /path/to/relic-vault   # the fresh binary
cargo install --path . # optional: replace the relic on PATH as well
```

Omit `--vault` to discover a vault from the current directory or its ancestors.
Use `--bind 127.0.0.1:7340` to choose another port; only loopback addresses are
accepted. Stop the server with Ctrl+C.

The UI provides:

- A vault overview with real entry statistics, effective confidence, and a
  needs-review count. Falling below the configured threshold does not change
  an entry's stored status.
- A paginated memory list with Unicode substring search and filters for
  status, kind, tag, source agent, and minimum effective confidence.
- Entry details, creation, and editing, backed by the same Markdown files as
  the CLI and MCP server.
- A raw YAML configuration editor with validation and conflict detection when
  the configuration has changed since it was loaded.
- Vault health checks and an action to rebuild the disposable SQLite index.
- A relation network view at `GET /api/graph`: the same derived knowledge graph
  the CLI and MCP tools serve, drawn as a force-directed network. Nodes are sized
  by relation count, an entry opens on click, and each line is labelled with its
  relation kind and weight — clicking it shows the evidence. Relation kinds can
  be switched off, a minimum weight can be raised to reveal the backbone rather
  than the full network, and anything omitted for legibility is stated rather
  than hidden.
- A memory topic sphere at the top of the overview — and only there; the banner
  is hidden on every other view. It projects `GET /api/brain` as a rotating
  pseudo-3D sphere whose primary nodes are **topics**: clusters of memories that
  already share a subject announced in their title, a non-marker tag, or
  distinctive title keywords. Satellites are that topic's tags, keywords, and
  source agents. Hovering a node shows its memory count, share, average
  effective confidence, memory-type composition, the tags and keywords it is
  built from, and example memories; clicking a topic searches the memory library
  for its subject, and clicking a tag or keyword applies the matching filter. The
  drawing is decorative — every primary node also exists as a focusable button,
  so the same information is reachable by keyboard and touch, and
  `prefers-reduced-motion` renders a single static frame. The projection is
  deterministic and derived from entry metadata only: no model, no network call,
  and no second source of truth. Topic, satellite, and keyword counts are capped
  for legibility and the banner states how many were not plotted, so a hidden
  memory is never silently implied away.
- A memory distribution view at the top of the memory library, aggregated by
  `GET /api/stats`: a memory-type ring, a lifecycle bar view, an effective
  confidence distribution (kernel density curve over a histogram, with the mean
  and the configured review threshold marked), how many memories survive each
  minimum confidence of 30%, 50%, 70%, and 90%, and ranked tag and source-agent
  frequency charts. The charts answer exactly the filters in force — kind,
  status, tag, source agent, minimum confidence, and search text — so every
  chart describes the memories in the list below it; the header states either
  the filtered count out of the vault total or that the whole vault is shown.
  Confidence is always the effective value after decay and expiry, lifecycle is
  the stored status rather than an inferred one, and the panel collapses to a
  single line with the preference kept in the browser, never in the vault.

The UI runs locally alongside the CLI and MCP server. Source-agent information
comes from entry metadata and does not represent live agent connections. The
same applies to the source-agent nodes in the memory brain.

## Connect an agent through MCP

Build the release binary and initialize a vault:

```bash
cargo build --release
./target/release/relic init ~/relic-vault
```

Relic's default MCP server uses standard input/output and never opens a network
port:

```bash
./target/release/relic mcp --vault ~/relic-vault
```

For clients that support Streamable HTTP, start the stateless JSON transport:

```bash
./target/release/relic mcp-http --vault ~/relic-vault
```

It listens only on `http://127.0.0.1:7337/mcp` by default. A quick protocol test:

```bash
curl http://127.0.0.1:7337/mcp \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"curl","version":"1"}}}'
```

`mcp-http` implements the Streamable HTTP transport: single-message requests return
one JSON response, batch requests (a JSON array) return a JSON array, notifications
are acknowledged with `202 Accepted`, and a client that sends only
`Accept: text/event-stream` receives its responses streamed as Server-Sent Events.
`GET /mcp` opens a long-lived SSE channel for future server-initiated messages.

After `initialize`, the server issues an `mcp-session-id` and validates it on
subsequent requests, rejecting an unknown or expired session with `404`. Clients
that omit the session id continue to work (the server is stateless-capable). The
server also serves OAuth 2.1 Authorization Server Metadata at
`/.well-known/oauth-authorization-server` and advertises `oauth` in its
`initialize` capabilities; the described token endpoints are scaffolding for a
remote deployment, while the configured Bearer token (`--bearer-token-env`) is
what actually protects the local endpoint.

Non-loopback binding requires Bearer authentication. Supply the secret through
an environment variable rather than a command-line value:

```bash
export RELIC_MCP_TOKEN='replace-with-a-long-random-token'
relic mcp-http --vault ~/relic-vault --bind 0.0.0.0:7337 \
  --bearer-token-env RELIC_MCP_TOKEN \
  --allow-origin https://agent.example.com
```

Configure Relic for an agent project and optionally add active memory guidance:

```bash
relic integrate codex \
  --vault ~/relic-vault \
  --project /path/to/project \
  --update-agents
```

Replace `codex` with any supported host:

```text
codex    .codex/config.toml       AGENTS.md
claude   .mcp.json                CLAUDE.md
cursor   .cursor/mcp.json         AGENTS.md
gemini   .gemini/settings.json    GEMINI.md
vscode   .vscode/mcp.json         AGENTS.md
dsh      $DSH_HOME/profiles/<profile>/cordis.patch.yml
         $DSH_HOME/profiles/<profile>/AGENTS.md
```

Each integration is idempotent, preserves unrelated settings and records the
correct source agent for new memories. `--update-agents` adds or refreshes only
Relic's managed instruction block. Use `--dry-run` to preview affected files
without writing them.

`--project` is optional. When omitted, Relic configures the agent for the current
user in its global configuration location:

```text
codex    ~/.codex/config.toml           ~/.codex/AGENTS.md
claude   ~/.claude.json                 ~/.claude/CLAUDE.md
cursor   ~/.cursor/mcp.json             User Rules are managed in Cursor settings
gemini   ~/.gemini/settings.json        ~/.gemini/GEMINI.md
vscode   ~/.copilot/mcp-config.json     ~/.copilot/copilot-instructions.md
dsh      $DSH_HOME/cordis.patch.yml       $DSH_HOME/AGENTS.md
```

Pass `--project /path/to/project` only when the integration should be scoped to
one project. Cursor does not expose a supported global rules file, so global
`--update-agents` configures its MCP server only; add the Relic guidance through
**Cursor Settings > Rules**.

DSH uses host and profile scopes rather than project-local MCP discovery. Pass
`--profile` to update that profile's MCP patch and instruction file:

```bash
relic integrate dsh --vault ~/relic-vault --profile tui
```

Omit `--profile` to update `$DSH_HOME/cordis.patch.yml` and
`$DSH_HOME/AGENTS.md`, which apply globally across profiles.

DSH currently does not automatically load `AGENTS.md` from a profile directory;
it loads `$DSH_HOME/AGENTS.md` and instructions from the session workspace. The
profile file is generated for explicit profile ownership, but omit `--profile`
when `--update-agents` must affect Agent behavior automatically.

Use `--dsh-home` only when DSH data lives somewhere other than `$DSH_HOME` or
`~/.dsh`.

To configure Codex manually instead, run:

```bash
codex mcp add relic -- \
  /absolute/path/to/Relic2077/target/release/relic \
  mcp --vault /absolute/path/to/relic-vault --source-agent codex
```

Or add a project-scoped `.codex/config.toml`:

```toml
[mcp_servers.relic]
command = "/absolute/path/to/Relic2077/target/release/relic"
args = ["mcp", "--vault", "/absolute/path/to/relic-vault", "--source-agent", "codex"]
required = true
default_tools_approval_mode = "writes"
```

### Claude Code

Register Relic for the current project:

```bash
claude mcp add --transport stdio --scope project relic -- \
  /absolute/path/to/Relic2077/target/release/relic \
  mcp --vault /absolute/path/to/relic-vault --source-agent claude-code
```

Run `/mcp` inside Claude Code to review and approve the project server. Use
`--scope user` instead if you want the same vault available in every project.

### Cursor

Add `.cursor/mcp.json` to the project, or use `~/.cursor/mcp.json` globally:

```json
{
  "mcpServers": {
    "relic": {
      "command": "/absolute/path/to/Relic2077/target/release/relic",
      "args": ["mcp", "--vault", "/absolute/path/to/relic-vault", "--source-agent", "cursor"]
    }
  }
}
```

Open **Cursor Settings > MCP** to enable `relic` and inspect its tools. Cursor
CLI uses the same configuration; run `agent mcp list` to check the connection.

### Gemini CLI

Add the server to the top-level `mcpServers` object in `.gemini/settings.json`
for the current project, or `~/.gemini/settings.json` globally:

```json
{
  "mcpServers": {
    "relic": {
      "command": "/absolute/path/to/Relic2077/target/release/relic",
      "args": ["mcp", "--vault", "/absolute/path/to/relic-vault", "--source-agent", "gemini-cli"]
    }
  }
}
```

Start or reload Gemini CLI, then run `/mcp` to verify the server and its tools.

### VS Code and GitHub Copilot

Add `.vscode/mcp.json` to the workspace:

```json
{
  "servers": {
    "relic": {
      "type": "stdio",
      "command": "/absolute/path/to/Relic2077/target/release/relic",
      "args": ["mcp", "--vault", "/absolute/path/to/relic-vault", "--source-agent", "github-copilot"]
    }
  }
}
```

Run **MCP: List Servers** from the Command Palette, start `relic`, and accept
the workspace trust prompt. A portable Agent Host configuration can instead be
stored in the workspace root as `.mcp.json`.

### DeepSeek Harness (DSH)

DSH connects to local MCP servers through its official
`@deepseek-ai/dsh-mcp-client`. Add this entry to the top-level array in your
profile's `cordis.patch.yml` (normally
`$DSH_HOME/profiles/<profile>/cordis.patch.yml`; `$DSH_HOME` defaults to
`~/.dsh`):

```yaml
- insert:
    - id: mcp-relic
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        transport: stdio
        serverName: relic
        command: /absolute/path/to/Relic2077/target/release/relic
        args:
          - mcp
          - --vault
          - /absolute/path/to/relic-vault
          - --source-agent
          - dsh
```

Start or reload DSH, then run `/mcp` to verify that the `relic` server exposes
twenty tools. The `--source-agent dsh` option attributes entries created without
an explicit `source_agent` to DeepSeek Harness.

The original eight knowledge tools are: `relic_search`, `relic_get_entry`,
`relic_list_entries`, `relic_create_entry`, `relic_update_entry`,
`relic_supersede_entry`, `relic_create_reflection`, and `relic_get_stats`.
Read-only and write operations carry MCP tool annotations so compatible clients
can apply appropriate approval policies.

## Sync with Git

The vault is a normal Git repository. `relic sync` moves a checked-out vault in
one deterministic step:

```bash
relic sync --remote origin --branch main --message "sync memory"
```

`sync` first ensures the directory is a Git repository (initialising one and an
initial commit if needed), commits any local Markdown changes, fetches and merges
the remote branch, then pushes. The remote name defaults to `origin`, and the
branch defaults to the currently checked-out branch, so the common case is simply
`relic sync`.

When a merge conflicts, Relic resolves it deterministically: the remote (theirs)
version becomes the canonical file and the local (ours) version is preserved
under `.relic/conflicts/<timestamp>/` — knowledge is never silently dropped. The
search index is rebuilt after every merge regardless of outcome.

Configure a default remote once in `.relic/config.yaml`:

```yaml
sync:
  mode: manual
  remotes:
    - name: origin
      url: git@github.com:you/relic-vault.git
```

The cheap, disposable SQLite index (`.relic/index.sqlite`) and embeddings are
already gitignored, so only your knowledge in Markdown is versioned.

## Reflection and analysis

Reflections are no longer static templates. `relic reflect` synthesises the
vault into a daily, weekly, or monthly draft that lists recent knowledge, any
detected contradictions, and candidate patterns to extract.

`relic analyze` reports those two signals without writing anything:

```bash
relic analyze
```

- **Contradiction detection** pairs active entries that share a subject tag but
  carry opposite claim polarity (positive vs. negative language such as "works"
  vs. "fails"). It flags a pair for review; it does not judge which is correct.
- **Pattern extraction** groups active entries by subject tag and proposes a
  reusable pattern when a group reaches a minimum size. `--write-patterns`
  materialises each proposal as a `pattern` entry.

Reflections can be triggered automatically instead of on demand:

```bash
relic reflect --period daily --auto --min-entries 5
```

`--auto` creates the reflection only when it does not already exist for that
period and the vault holds at least `--min-entries` entries (default 5).

## Knowledge graph and vector relations

`relic graph` derives a relation network over your memories. It has two layers
that answer different questions:

- a **vector layer** answers *what is near this memory*: every entry becomes a
  sparse vector in a hashed feature space, weighted by inverse document
  frequency, and compared by cosine similarity;
- a **logic layer** answers *what is the named relation*: typed edges, each
  carrying its derivation and the concrete evidence behind it.

Both are derived from Markdown and both are disposable. Delete
`.relic/graph.json` and `.relic/embeddings/` and they rebuild.

```bash
relic graph build          # rebuild the vector layer and the relation network
relic graph stats          # size, components, hubs, isolated memories
relic graph explain <a> <b>  # every relation between two memories, with evidence
relic graph path <a> <b>     # the strongest chain of relations between them
relic graph similar <id>     # nearest memories, with the vocabulary that relates them
relic graph export --format mermaid
```

### Relation kinds

| Kind | Derivation | Rule |
| --- | --- | --- |
| `link` | explicit | a `links` reference in the front matter |
| `supersedes` | explicit | version history |
| `tag` | statistical | Jaccard overlap of subject tags |
| `semantic` | statistical | vector neighbours above a calibrated cosine floor |
| `contradicts` | logical | a shared subject with opposite, decisive claims |
| `corroborates` | logical | a shared subject with matching, decisive claims |

Inferred edges are never presented as facts. Each one stores the reason it
exists, so `relic graph explain` reads like an argument rather than a score:

```text
semantic      0.291  statistical
  cosine 0.291 in 8192 hashed dimensions; nearest vocabulary: retrieval, chunk, chunking, 512, across
```

### Why there is no embedding model

Relic cannot call a hosted embedding API without breaking its own contract:
plain text owns the truth, everything derived is rebuildable, and a query never
needs a network or a model. So the vector layer is a real sparse retrieval
geometry built locally — sublinear term frequency, BM25-style inverse document
frequency, signed feature hashing, L2 normalisation — and it is deterministic:
the same Markdown always produces the same vectors, which is what makes the
graph testable.

Hashing normally costs explainability, because a dimension no longer knows which
word it came from. Relic records the most frequent tokens per occupied dimension
for exactly this reason, which is why a semantic edge can name the vocabulary
that carried it.

Two tokenizer decisions were measured rather than assumed: folding regular
English plurals (`chunks` → `chunk`) raised the weakest related pair from 0.183
to 0.289 cosine, and CJK memories are indexed as characters plus adjacent pairs
because those scripts have no whitespace word boundaries.

### Thresholds are measured, not guessed

Over memories that share a subject, cosine similarity runs 0.289–0.328; over
unrelated memories it never exceeds 0.035. The semantic floor of 0.08 sits in
that gap with more than a factor of two of margin on both sides, and
`tests/graph_calibration.rs` asserts the separation — so a change that destroys
it fails loudly with numbers instead of quietly filling the graph with noise.

Small vaults are legitimately sparse: inverse document frequency needs a corpus
before it can separate subjects. `relic graph build` says so in its notes rather
than leaving you to wonder.

### Search modes

```bash
relic search "512 token chunks" --mode keyword   # literal, precise (default)
relic search "分块策略" --mode semantic           # vector similarity
relic search "chunks prose tokens alpine" --mode hybrid
```

Keyword and vector scores are not comparable — BM25 is unbounded, cosine is not
— so `hybrid` fuses the two *rankings* with Reciprocal Rank Fusion rather than
normalising scores against each other. A memory ranked well by both layers
rises; a memory only one layer can see still appears.

The layers fail in opposite directions, which is why having both matters: FTS5
cannot find "chunking strategy" when you type "chunk size", and it treats a
Chinese query as an exact contiguous run, while the vector layer finds related
memories regardless of wording but cannot express an exact identifier.

### Agents can traverse it too

Five read-only MCP tools expose the graph to any connected agent:
`relic_find_similar`, `relic_graph_neighbors`, `relic_graph_path`,
`relic_explain_relation`, and `relic_graph_stats`. They carry `readOnlyHint`, so
traversing relations can never change the vault. `relic_search` gains a `mode`
argument, defaulting to `keyword` so existing clients keep their behaviour.

### Configuration

```yaml
graph:
  dimensions: 8192
  semantic_top_k: 8
  semantic_min_similarity: 0.08
  tag_min_jaccard: 0.34
  corroborate_min_similarity: 0.15
  max_edges_per_node: 24
```

These are thresholds on derived data: changing one changes what `relic graph
build` infers and never rewrites a Markdown file. The browser dashboard has a
**关系网络** (relation network) view that renders the same derived graph, where
every line is clickable to show its evidence.

The [architecture and design notes](docs/knowledge-graph.md) (Chinese) cover the
algorithm, the deliberate omissions, the cost model, and the known limits.

## Configuration and daemon

`.relic/config.yaml` is the stable, validated contract for a vault. `relic
doctor` checks the schema version and the constraints on every field (bounded
confidence and decay values, a supported `sync.mode`, and well-formed, unique
sync remotes). Unknown fields are ignored, so a newer config does not break an
older binary.

Configure default sync remotes once and `relic sync` then needs no arguments — it
commits local changes, pulls and merges the first configured remote, and pushes:

```yaml
sync:
  mode: manual
  remotes:
    - name: origin
      url: git@github.com:you/relic-vault.git
```

When `--remote` is omitted, Relic adds the first configured remote if it is not
already known and syncs it. This is the multi-device workflow: point each device
at the same vault, configure the same remote, and run `relic sync`.

`relic watch` keeps a vault warm as a maintenance daemon — it rebuilds the search
index and auto-reflects when the trigger is met:

```bash
relic watch                 # loop every 60 seconds
relic watch --once          # run a single maintenance pass and exit
relic watch --interval 120 --reflect-period daily --min-entries 5
```

## Principles

1. **Plain text owns the truth.** The database can always be deleted and rebuilt.
2. **History beats deletion.** Superseded knowledge stays available to Git and future reflection.
3. **Confidence is explicit.** Entries state how certain and how fresh they are.
4. **Agents are replaceable.** The vault is not coupled to any model or vendor.
5. **Inference is labelled.** A relation the vault inferred states how it was
   derived and what evidence supports it; it is never presented as something you
   declared.

## Layout

```text
.relic/          configuration, schema, and disposable local index
                 (search index, vector layer, and knowledge graph all derive
                 from the Markdown below and can be deleted at any time)
entries/         core knowledge
reflections/     daily, weekly, and monthly reviews
patterns/        reusable patterns extracted from multiple entries
decisions/       ADR-style decisions
sources/         original references and conversations
attachments/     images and files
AGENTS.md         instructions for any agent entering the vault
```

## Roadmap

- **0.2 (complete):** update/supersede CLI commands, decay calculation, richer filters
- **0.3 (complete):** Streamable HTTP transport (JSON + SSE streaming, batches), optional Bearer authentication, `mcp-session-id` sessions, and OAuth 2.1 discovery metadata
- **0.4 (complete):** `relic sync` git commit/pull/push with deterministic conflict resolution that preserves local versions
- **0.5 (complete):** reflection synthesis with automatic `--auto` triggers, contradiction detection, and pattern extraction
- **1.0 (complete):** config schema validation (`relic doctor`), `relic watch` maintenance daemon, and config-driven sync remotes for multi-device workflow
- **1.1 (complete):** the knowledge graph — a deterministic local vector layer, typed evidence-carrying relations, hybrid retrieval, graph traversal over MCP, and a relation view in the dashboard

## Durable experience inbox

Capture structured experiences with `relic capture experience.json`, prepare review drafts with `relic queue work` or `relic watch`, and publish reviewed Markdown with `relic queue review <capture-UUID> review.json`. Capture retries are idempotent within a vault; interrupted processing and publication can resume. Requires Rust 1.89+.

Six additional MCP tools expose this flow: `relic_capture`, `relic_list_captures`, `relic_get_capture`, `relic_process_captures`, `relic_review_capture`, and `relic_retry_capture`. Source events and accepted Markdown can use existing Git sync; local queue state must be backed up separately. Capture produces template drafts by default; an optional local command adapter can generate model drafts, with evidence still requiring review. Automatic Git sync is separately opt-in through the daemon described below. See the [workflow and recovery guide (Chinese)](docs/memory-pipeline.md).

### Lifecycle hooks and background sync

Codex hooks and a native DSH adapter retrieve relevant memories before a task and durably capture the final turn response for review. Run `relic daemon --vault /absolute/vault` independently to prepare candidates and, with `sync.mode: auto` and a configured remote, synchronize the vault. Conflicts pause unattended sync without choosing either version. The dashboard now exposes hook activity, candidate review, and retries. See the [installation and operation guide (Chinese)](docs/automation.md).

Optional model-command distillation is configured locally with `relic distiller configure`. Model drafts remain review-only; `relic queue redistill` / `relic_redistill_capture` requeues unreviewed candidates. See the [adapter protocol](docs/memory-pipeline.md#第二阶段可配置模型命令蒸馏).

## Import memories Codex already distilled

When Codex memory is enabled (`[features] memories = true`), Codex maintains its
own two-stage memory product: `MEMORY.md` groups cross-session knowledge by task
group with user preferences, reusable knowledge and failure lessons. Those
bullets are already distilled memory points, so Relic can transport them instead
of re-deriving them:

```bash
cd ~/relic-vault
relic import-codex-memory --dry-run
relic import-codex-memory --exclude-task-group Relic2077
```

The importer never calls a model and never runs on its own: it reads
`$CODEX_HOME/memories/MEMORY.md` only when you ask, writes `entries/inbox/codex-memory-*.md`,
and tags every entry `codex-memory` / `unverified` at 0.6 confidence. Identity is
derived from the bullet text, so re-running is a no-op and a rewritten bullet
becomes a new entry rather than a silent overwrite. See the
[import contract (Chinese)](docs/memory-pipeline.md#导入-codex-已提炼的记忆).
