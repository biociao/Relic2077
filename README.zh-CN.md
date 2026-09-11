<div align="center">

# Relic2077

### 检索增强型本地智能缓存

[English](README.md) | **简体中文**

![Relic2077 Agent Memory Core](assets/relic2077-agent-memory-core.png)

> Secure Your Soul

</div>

---

Relic2077 是一个本地优先、Git 原生的智能体经验中枢，   
让你在推陈出新、争夺用户入口的AI Agent工具的使用过程中延续同一份记忆。
它为 Codex、Claude、Cursor、DeepSeek Harness（DSH）以及其他兼容 MCP 的 Agent 
提供用户完全自有的统一记忆，拒绝将个人知识绑定到任何模型或厂商。   
Markdown 是唯一事实来源；SQLite 只是可以随时丢弃并重建的搜索索引。  
你的知识始终可读、可迁移、可重建，并且永远属于你。

</div>

## 当前里程碑

当前仓库包含 CLI、MCP 集成和本地浏览器 UI：

- 初始化带有完整 Schema 的可迁移知识库；
- 创建、读取、列出和全文检索知识条目；
- 管理知识、模式、决策、来源、附件与反思；
- 生成每日、每周或每月反思草稿；
- 随时从 Markdown 完整重建 SQLite FTS5 索引；
- 使用 `relic doctor` 检查知识库健康状态；
- 为记忆建立知识图谱：带类型与证据的关系网，加上完全本地的矢量空间，两者都从 Markdown 推导、都可随时删除重建；
- 通过本地 STDIO MCP Server 向 Codex、Claude、Cursor 等 Agent 开放知识库；
- 在浏览器中查看记忆、编辑条目与配置，并检查知识库健康状态。

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
relic search "分块策略" --mode hybrid
relic update <条目-ID> --confidence 0.9 --tags rag,verified
relic supersede <旧条目-ID> <新条目-ID>
relic list --status active
relic reflect --period weekly
relic graph build
relic graph stats
relic graph similar <条目-ID>
relic stats
relic doctor
```

## 打开本地 UI

使用已安装的命令，或直接从本仓库启动：

```bash
relic ui --vault /path/to/relic-vault
# 在本仓库目录中运行：
cargo run -- ui --vault /path/to/relic-vault
```

在现代浏览器中打开 [http://127.0.0.1:7338](http://127.0.0.1:7338)。
UI 由本地 Rust 服务提供，页面资源内嵌在二进制中，可在 Windows、macOS 和
Linux 上使用同一套界面，无需 Node.js、CDN 或额外的前端运行环境。
修改 `ui/` 中的源文件后，需要重新构建二进制。
省略 `--vault` 时，从当前目录及其上级目录查找知识库。
使用 `--bind 127.0.0.1:7340` 更换端口；仅允许绑定本机回环地址。
按 Ctrl+C 停止服务。

UI 提供：

- 知识库总览：真实条目统计、有效置信度和待复核数量。置信度低于配置阈值
  只会提示复核，不会自动修改条目中保存的状态。
- 分页记忆列表：支持 Unicode 子串搜索，以及状态、类型、标签、来源 Agent
  和最低有效置信度筛选。
- 条目详情、新建与编辑：与 CLI、MCP Server 共用同一份 Markdown 文件。
- 原始 YAML 配置编辑：保存前校验配置，并检测加载后发生的配置冲突。
- 知识库健康检查与 SQLite 搜索索引重建。
- 「记忆主题球形图」：只出现在「记忆总览」首页顶部，其他页面不显示。它把
  `GET /api/brain` 投影成一个可旋转的伪 3D 球形：主节点是**主题**（由标题里
  已声明的课题、非标记标签、或标题关键词聚类而成的记忆簇），伴星是该主题的
  标签、关键词与来源 Agent。悬停节点可查看记忆数量、占比、平均有效置信度、
  记忆类型构成、组成它的标签与关键词，以及代表记忆；点击主题＝在记忆库中按该
  主题检索，点击标签/关键词＝应用对应筛选。画布本身只是可视化：每个主节点同时
  是可聚焦按钮，键盘与触屏获得相同信息，`prefers-reduced-motion` 下只渲染一帧。
  该投影完全由条目元数据推导，不调用模型、不发外部请求。主题、伴星与关键词数量
  设有上限以保证可读性，未入图的数量会在横幅中写明，不会悄悄隐去记忆。

UI 在本机运行，可与 CLI、MCP Server 并用。来源 Agent 信息来自条目元数据，
不表示 Agent 当前在线或已连接；记忆图谱中的来源节点同理。

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

为 Agent 项目配置 Relic，并可选择加入主动使用长期记忆的规则：

```bash
relic integrate codex \
  --vault ~/relic-vault \
  --project /path/to/project \
  --update-agents
```

可将 `codex` 替换为任一受支持的宿主：

```text
codex    .codex/config.toml       AGENTS.md
claude   .mcp.json                CLAUDE.md
cursor   .cursor/mcp.json         AGENTS.md
gemini   .gemini/settings.json    GEMINI.md
vscode   .vscode/mcp.json         AGENTS.md
dsh      $DSH_HOME/profiles/<profile>/cordis.patch.yml
         $DSH_HOME/profiles/<profile>/AGENTS.md
```

所有集成都可幂等执行，保留无关配置，并为新记忆记录正确的来源 Agent。
`--update-agents` 只会添加或刷新 Relic 管理的指令区块。可传入 `--dry-run`
预览将受影响的文件而不实际写入。

`--project` 是可选参数。省略时，Relic 会写入当前用户的 Agent 全局配置：

```text
codex    ~/.codex/config.toml           ~/.codex/AGENTS.md
claude   ~/.claude.json                 ~/.claude/CLAUDE.md
cursor   ~/.cursor/mcp.json             全局 User Rules 需在 Cursor 设置中管理
gemini   ~/.gemini/settings.json        ~/.gemini/GEMINI.md
vscode   ~/.copilot/mcp-config.json     ~/.copilot/copilot-instructions.md
dsh      $DSH_HOME/cordis.patch.yml       $DSH_HOME/AGENTS.md
```

只有希望集成局限于单个项目时，才传入 `--project /path/to/project`。Cursor
没有受支持的全局规则文件，因此全局模式下的 `--update-agents` 只配置 MCP；
Relic 指令需要通过 **Cursor Settings > Rules** 添加。

DSH 使用宿主和 profile 作用域，而不是自动发现项目级 MCP 配置。传入
`--profile` 时会更新该 profile 的 MCP patch 和指令文件：

```bash
relic integrate dsh --vault ~/relic-vault --profile tui
```

省略 `--profile` 时则更新 `$DSH_HOME/cordis.patch.yml` 和
`$DSH_HOME/AGENTS.md`，作为所有 profile 共用的全局配置。

DSH 当前不会自动加载 profile 目录中的 `AGENTS.md`；它只加载
`$DSH_HOME/AGENTS.md` 和会话工作区内的指令文件。profile 文件用于明确归属，
如果希望 `--update-agents` 自动影响 Agent 行为，请省略 `--profile`。

只有当 DSH 数据不位于 `$DSH_HOME` 或 `~/.dsh` 时才需要传入 `--dsh-home`。

如需手动配置 Codex，可以运行：

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

启动或重新加载 DSH，然后运行 `/mcp`，确认 `relic` Server 已公开二十个工具。
`--source-agent dsh` 会将没有显式指定 `source_agent` 的新条目归因到
DeepSeek Harness。

原有八个知识库工具：`relic_search`、`relic_get_entry`、
`relic_list_entries`、`relic_create_entry`、`relic_update_entry`、
`relic_supersede_entry`、`relic_create_reflection` 和 `relic_get_stats`。
读写操作均带有 MCP 工具注解，因此兼容客户端可以应用适当的审批策略。

## 设计原则

1. **纯文本掌握事实。** 数据库可以随时删除并重新构建。
2. **历史优于删除。** 被取代的知识仍保留在 Git 和未来的反思中。
3. **显式表达置信度。** 条目明确说明其可信程度和新鲜程度。
4. **Agent 可以替换。** 知识库不与任何模型或厂商绑定。
5. **推断必须标注。** 系统推断出的关系会说明推导方式与证据，绝不冒充你亲自声明的关系。

## 知识图谱与矢量关系网

`relic graph` 为记忆推导出一张关系网，它由互补的两层组成：

- **矢量层**回答「哪些记忆彼此接近」：每条记忆成为哈希特征空间中的一个稀疏向量，以逆文档频率加权，用余弦相似度比较；
- **逻辑层**回答「它们之间是什么关系」：带类型的关系边，每条都记录自己的推导方式与具体证据。

两层都从 Markdown 推导，都可以随时删除重建。

```bash
relic graph build            # 重建矢量层与关系网
relic graph stats            # 规模、连通分量、中心与孤立记忆
relic graph explain <a> <b>  # 两个记忆之间的全部关系及证据
relic graph path <a> <b>     # 两者之间最强的推理链
relic graph similar <id>     # 最接近的记忆，以及支撑该关系的词表
relic graph export --format mermaid
```

关系类型：`link`（显式链接）、`supersedes`（版本替代）、`tag`（共同主题）、`semantic`（语义相近）、`contradicts`（结论冲突）、`corroborates`（结论互证）。推断出的边从不伪装成事实——每条边都保存它存在的理由，因此 `relic graph explain` 读起来像论证而不是分数：

```text
semantic      0.291  statistical
  cosine 0.291 in 8192 hashed dimensions; nearest vocabulary: retrieval, chunk, chunking, 512, across
```

**为什么不调用 embedding 模型：** Relic 的契约是纯文本掌握事实、派生产物可重建、查询不需要网络与模型。因此矢量层是本地构建的真实稀疏检索几何（亚线性词频、BM25 风格 IDF、带符号特征哈希、L2 归一化），并且是确定性的：同一份 Markdown 永远产生相同的向量，这正是不变量可测试的前提。哈希的代价通常是失去可解释性，因此语料会记录每个被占用维度的代表词——这就是语义边能说出「是哪些词让它相近」的原因。

**阈值是量出来的：** 共享主题的记忆对余弦为 0.289–0.328，无关记忆对不超过 0.035，语义下限取 0.08，两侧都有两倍以上余量，并由 `tests/graph_calibration.rs` 守护这一分离。语料过小时图谱会稀疏，这是正确行为，`graph build` 会在 notes 里说明原因。

**混合检索：** `relic search --mode hybrid` 用倒数排名融合（RRF）只融合两层各自的**排名**，因为 BM25 无上界、余弦在 [0,1]，分数本身不可比。两层恰好互补：FTS5 无法在你输入 "chunk size" 时找到 "chunking strategy"，也会把中文查询当作精确连续串；矢量层则不依赖措辞，但无法表达精确标识符。

**Agent 也能遍历：** 五个只读 MCP 工具（`relic_find_similar`、`relic_graph_neighbors`、`relic_graph_path`、`relic_explain_relation`、`relic_graph_stats`）均带 `readOnlyHint`——遍历关系永远不会改动知识库。

架构与取舍详见[知识图谱设计说明](docs/knowledge-graph.md)。

## 目录结构

```text
.relic/          配置、Schema 和可丢弃的本地索引
                 （搜索索引、矢量层与知识图谱都由下方 Markdown 推导，可随时删除）
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
- **1.1：** 知识图谱——确定性本地矢量层、带证据的类型化关系、混合检索、MCP 图遍历与仪表盘关系视图

## 持久化经验队列

新增 capture、候选复核和可恢复 worker，支持 CLI、MCP 和 watch。参见[经验到知识的完整流程](docs/memory-pipeline.md)。新增工具为 `relic_capture`、`relic_list_captures`、`relic_get_capture`、`relic_process_captures`、`relic_review_capture`、`relic_retry_capture`。

### 自动化钩子与后台同步

现支持 Codex 任务前检索、轮次结束采集，以及 DSH 原生适配器。`relic daemon --vault /absolute/vault` 独立处理候选；配置 `sync.mode: auto` 和远端后自动同步，冲突时暂停。UI 的“自动化与同步”页面可查看事件、审核候选、重试失败任务。安装与边界说明见[自动化指南](docs/automation.md)。

模型命令蒸馏可通过 `relic distiller configure` 在本机启用；候选仍需复核。已有候选可用 `relic queue redistill` 或 `relic_redistill_capture` 重新处理。详见[模型蒸馏协议](docs/memory-pipeline.md#第二阶段可配置模型命令蒸馏)。
