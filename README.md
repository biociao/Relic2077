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

This repository contains the Phase 0 CLI plus the first MCP integration:

- initialize a portable vault with a documented schema;
- create, read, list, and full-text search knowledge entries;
- organize knowledge, patterns, decisions, sources, attachments, and reflections;
- generate daily, weekly, or monthly reflection drafts;
- rebuild the entire SQLite FTS5 index from Markdown at any time;
- validate vault health with `relic doctor`.
- expose the vault to Codex, Claude, Cursor, and other agents through a local
  STDIO MCP server.

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
relic watch --once
relic stats
relic doctor
relic sync
```

`relic list` displays effective confidence. Relic continuously decays the stored
confidence from `last_verified` using `confidence × e^(-decay_rate × years)`;
expired entries have an effective confidence of zero. Updating an entry's
confidence also refreshes `last_verified`. List filters include `--kind`,
`--status`, `--tags`, `--source-agent`, `--min-confidence`, and
`--max-confidence`.

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
eight tools. The `--source-agent dsh` option attributes entries created without
an explicit `source_agent` to DeepSeek Harness.

The server publishes eight tools: `relic_search`, `relic_get_entry`,
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

## Layout

```text
.relic/          configuration, schema, and disposable local index
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
