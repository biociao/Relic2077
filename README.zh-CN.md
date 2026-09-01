<div align="center">

# Relic2077

### 检索增强型本地智能缓存

[English](README.md) | **简体中文**

![Relic2077 Agent Memory Core](assets/relic2077-agent-memory-core.png)

> Secure Your Soul

---

Relic2077 是一个本地优先、Git 原生的智能体经验中枢，让你在推陈出新、争夺用户入口的AI Agent工具的使用过程中延续同一份记忆。它为 Codex、Claude、Cursor、DeepSeek Harness（DSH）以及
其他兼容 MCP 的 Agent 提供用户完全自有的统一记忆，拒绝将个人知识绑定到任何模型或厂商。

Markdown 是唯一事实来源；SQLite 只是可以随时丢弃并重建的搜索索引。
你的知识始终可读、可迁移、可重建，并且永远属于你。

## 当前里程碑

当前仓库包含 Phase 0 CLI 和首个 MCP 集成：

- 初始化带有完整 Schema 的可迁移知识库；
- 创建、读取、列出和全文检索知识条目；
- 管理知识、模式、决策、来源、附件与反思；
- 生成每日、每周或每月反思草稿；
- 随时从 Markdown 完整重建 SQLite FTS5 索引；
- 使用 `relic doctor` 检查知识库健康状态；
- 通过本地 STDIO MCP Server 向 Codex、Claude、Cursor 等 Agent 开放知识库。

Git 同步、置信度演化、远程 MCP 传输和专用 Agent 适配器属于后续里程碑。
现有存储格式已经为这些能力做好准备。

## 安装

```bash
cargo install --path .
```

安装后会提供一个名为 `relic` 的命令。

## 快速开始

```bash
relic init ~/relic-vault
cd ~/relic-vault

relic add "RAG 分块策略" \
  --content "文本场景使用 512–1024 token 分块，并针对实际语料进行验证。" \
  --tags rag,chunking \
  --confidence 0.82 \
  --source-agent codex

relic search "分块"
relic update <条目-ID> --confidence 0.9 --tags rag,verified
relic supersede <旧条目-ID> <新条目-ID>
relic list --status active
relic reflect --period weekly
relic stats
relic doctor
```

## 通过 MCP 连接 Agent

构建 release 二进制并初始化知识库：

```bash
cargo build --release
./target/release/relic init ~/relic-vault
```

Relic MCP Server 使用标准输入输出，不会开放网络端口：

```bash
./target/release/relic mcp --vault ~/relic-vault
```

添加到 Codex：

```bash
codex mcp add relic -- \
  /absolute/path/to/Relic2077/target/release/relic \
  mcp --vault /absolute/path/to/relic-vault --source-agent codex
```

也可以添加项目级 `.codex/config.toml`：

```toml
[mcp_servers.relic]
command = "/absolute/path/to/Relic2077/target/release/relic"
args = ["mcp", "--vault", "/absolute/path/to/relic-vault", "--source-agent", "codex"]
required = true
default_tools_approval_mode = "writes"
```

### Claude Code

为当前项目注册 Relic：

```bash
claude mcp add --transport stdio --scope project relic -- \
  /absolute/path/to/Relic2077/target/release/relic \
  mcp --vault /absolute/path/to/relic-vault --source-agent claude-code
```

在 Claude Code 中运行 `/mcp`，检查并批准这个项目级 Server。如果希望所有
项目共用同一知识库，将 `--scope project` 改为 `--scope user`。

### Cursor

在项目中添加 `.cursor/mcp.json`；如需全局使用，则添加
`~/.cursor/mcp.json`：

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

打开 **Cursor Settings > MCP**，启用 `relic` 并检查其工具。Cursor CLI
共用这份配置，可运行 `agent mcp list` 检查连接状态。

### Gemini CLI

将 Server 添加到当前项目 `.gemini/settings.json` 的顶层 `mcpServers`
对象中；如需全局使用，则编辑 `~/.gemini/settings.json`：

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

启动或重新加载 Gemini CLI，然后运行 `/mcp` 检查 Server 及其工具。

### VS Code 与 GitHub Copilot

在工作区中添加 `.vscode/mcp.json`：

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

从命令面板运行 **MCP: List Servers**，启动 `relic`，并接受工作区信任
提示。如需让 Agent Host 直接读取可移植配置，也可以将配置保存为工作区
根目录下的 `.mcp.json`。

### DeepSeek Harness（DSH）

DSH 通过官方 `@deepseek-ai/dsh-mcp-client` 连接本地 MCP Server。将以下配置
加入当前 profile 的 `cordis.patch.yml` 顶层数组中。该文件通常位于
`$DSH_HOME/profiles/<profile>/cordis.patch.yml`，而 `$DSH_HOME` 默认是 `~/.dsh`：

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

启动或重新加载 DSH，然后运行 `/mcp`，确认 `relic` Server 已公开八个工具。
`--source-agent dsh` 会将没有显式指定 `source_agent` 的新条目归因到
DeepSeek Harness。

Server 提供八个工具：`relic_search`、`relic_get_entry`、
`relic_list_entries`、`relic_create_entry`、`relic_update_entry`、
`relic_supersede_entry`、`relic_create_reflection` 和 `relic_get_stats`。
读写操作均带有 MCP 工具注解，因此兼容客户端可以应用适当的审批策略。

## 设计原则

1. **纯文本掌握事实。** 数据库可以随时删除并重新构建。
2. **历史优于删除。** 被取代的知识仍保留在 Git 和未来的反思中。
3. **显式表达置信度。** 条目明确说明其可信程度和新鲜程度。
4. **Agent 可以替换。** 知识库不与任何模型或厂商绑定。

## 目录结构

```text
.relic/          配置、Schema 和可丢弃的本地索引
entries/         核心知识
reflections/     每日、每周和每月回顾
patterns/        从多个条目中提取的可复用模式
decisions/       ADR 风格的决策记录
sources/         原始参考资料和对话
attachments/     图片和文件
AGENTS.md         提供给所有 Agent 的知识库说明
```

## 路线图

- **0.2：** update/supersede CLI 命令、置信度衰减计算和更丰富的筛选条件
- **0.3：** Streamable HTTP MCP 传输和可选身份认证
- **0.4：** Git 同步和冲突解决
- **0.5：** 反思触发、矛盾检测和模式提取
- **1.0：** 稳定存储 Schema、适配器、守护进程和多设备工作流
