# 持久化经验与知识沉淀（第一阶段）

Relic 现在提供一个可恢复的经验处理流程：

```text
capture → pending → processing → needs_review → accepted → Markdown 知识
                         │              └────→ rejected
                         └→ failed → retry → pending
```

`capture` 接收结构化经验；`queue work` 或 `watch` 生成模板草稿；用户或 Agent
检查证据和相关知识后，通过 `queue review` 收录或拒绝。候选草稿不进入正式知识
检索，也不参与反思和模式提取。此阶段没有调用 LLM、自动验证证据或自动判定语义重复。

## 快速使用

需要 Rust 1.89 或更新版本（使用标准库的进程级文件锁）。构建后进入知识库目录：

```sh
cargo install --path /path/to/Relic2077
cd ~/relic-vault
```

创建 `experience.json`（路径也可以放在知识库外）：

```json
{
  "event_id": "search-recovery-1",
  "project": "my-project",
  "session_id": "session-20260907",
  "source_agent": "codex",
  "title": "搜索索引丢失后的恢复",
  "context": "重启后本地搜索索引丢失，Markdown 知识仍完整。",
  "action": "从 Markdown 重新构建索引。",
  "outcome": "搜索恢复，知识条目没有丢失。",
  "evidence": ["重启与重建索引的回归测试通过"],
  "tags": ["search", "recovery"]
}
```

```sh
relic capture /path/to/experience.json
relic queue work --capture-id <capture-UUID>00
relic queue list
relic queue get <capture-UUID>
```

`capture` 返回的 `id` 是后续命令使用的 capture UUID；它与调用方提供的 `event_id`
不同。CLI 未提供 `source_agent` 时默认为 `cli`，MCP 默认为连接的 Agent 来源。

检查候选和来源后，创建 `review.json`：

```json
{
  "decision": "accept",
  "reason": "已核对回归测试，结论适用于索引丢失且 Markdown 完整的情况。",
  "knowledge": {
    "title": "索引丢失时从 Markdown 恢复搜索",
    "content": "当索引丢失但 Markdown 完整时，运行 relic reindex 重建搜索索引。验证依据：重启恢复回归测试。",
    "kind": "lesson",
    "confidence": 0.9,
    "tags": ["search", "recovery"]
  }
}
```

```sh
relic queue review <capture-UUID> /path/to/review.json
relic search "索引"
```

`knowledge` 可省略，表示接受当前模板草稿；模板的初始置信度为 `0.5`，证据明确标注
为提交者报告、未独立验证。`kind` 支持 `knowledge`、`lesson`、`decision`、`pattern`。
接受会创建一条新知识，自动附加来源文件、项目、会话、事件 ID 和复核原因。
若已有知识适合更新，先使用现有 update/supersede 接口完成修订，再拒绝重复候选，
在原因中写明目标条目 ID；本阶段不会自动合并或替代已有知识。

拒绝示例：

```json
{"decision":"reject","reason":"与条目 relic-xxx 重复，已将证据补充到该条目。"}
```

## 持续处理与 MCP

```sh
relic watch --interval 60
relic watch --once
relic queue retry <failed-capture-UUID>
relic queue work
```

`watch` 每轮最多处理 100 个事件，再维护索引和周期反思。`queue work --limit N`
支持 1–1000。失败事件保留错误和尝试次数，修复原因后显式 retry；后台不会无限重试。
处理中的进程退出后，下一轮自动继续。已保存的复核决定也会在下一轮完成写回。
`failed` 状态需要显式 retry，包含在复核恢复过程中再次失败的情况。

新增 MCP 工具：

| 工具 | 用途 |
|---|---|
| `relic_capture` | 接收上述经验字段并持久化 |
| `relic_list_captures` | 列出本地队列及候选 |
| `relic_get_capture` | 按 `capture_id` 读取来源和候选 |
| `relic_process_captures` | 生成草稿、恢复中断任务 |
| `relic_review_capture` | 提交 `capture_id` 和 `review` 对象 |
| `relic_retry_capture` | 将失败事件重新排队 |

重新运行 `relic integrate <agent> ... --update-agents` 可以更新宿主指令。
捕获仍由 Agent 主动调用 MCP；没有新增宿主会话结束钩子或自动扫描对话历史。

## 持久化与恢复约定

- 原始经验：`sources/captures/<UUID>.json`，版本 1，写入后不修改，可随 Git 同步。
- 本地状态、候选、复核记录：`.relic/queue/<UUID>.json`。这是持久数据，需要备份，
  **不能像搜索索引一样随意删除**；目录通过自身 `.gitignore` 排除 Git，同样兼容旧知识库。
- 正式知识：现有 `entries/`、`decisions/`、`patterns/` 内的 Markdown；来源路径存入 `links`。
- 索引：`.relic/index.sqlite`，始终可从正式 Markdown 重建。

接收成功前执行临时文件写入、文件同步、原子重命名和父目录同步。
队列使用操作系统文件锁，在本地文件系统上串行处理并发写入；进程退出会释放锁。
不要删除 `worker.lock`。网络文件系统的锁和同步语义不在此阶段保证范围内。

同一知识库内，`source_agent + project + session_id + event_id` 是幂等键：
相同键与内容返回同一 capture；相同键但不同内容报错。修订经验应使用新 `event_id`。
接收后若响应丢失，重发原请求即可。单条经验的 JSON 序列化大小上限为 256 KiB。

收录使用固定知识 ID 和路径，并先持久化复核决定。如果 Markdown 已写入而索引或状态
更新中断，恢复时复用现有条目，不会再次创建。发生错误后重试同一份复核决定，或由
下一轮 worker 恢复；已保存的决定不允许换成另一个决定。

## 同步边界与后续阶段

`relic sync` 继续同步来源文件和正式 Markdown，接收端重建索引。
本地领取状态、失败计数、未收录候选和拒绝决定不通过 Git 同步。
同步来的来源若已有对应正式条目，worker 会恢复为 accepted；否则生成本地待复核草稿。
因此另一设备不会知道本机的拒绝决定。同一逻辑事件若在两台离线设备分别首次提交，
可能产生不同 UUID；当前幂等保证限于同一知识库的串行提交。

本阶段没有增加自动 Git 推送、分布式任务领取、跨设备审核同步、语义合并、LLM 蒸馏
或使用反馈评分。现有 Git 冲突处理策略也未改变。下一阶段应独立设计同步审核记录和
模型处理接口，避免将多个 worker 的本地队列状态直接通过 Git 合并。

提交者应只提供可保留的经验和证据摘要，事先移除秘密与原始日志。本阶段没有自动
脱敏器；内容落盘后可能随来源文件进行 Git 同步。

## 自动化接入

Codex/DSH 钩子、独立 worker 和可选自动 Git 同步已另行实现；参见[自动化指南](automation.md)。上面的采集与审核契约不变，自动钩子不会跳过审核或触发 LLM。

## 导入 Codex 已提炼的记忆

Codex 开启 `[features] memories = true`（或 `[memories] generate_memories = true`）后，
会自行维护一套两阶段记忆：`$CODEX_HOME/memories/MEMORY.md` 是二级综合产物，按主题
（Task Group）分组，每组带 `scope` / `applies_to`，组内 bullet 分「User preferences /
Reusable knowledge / Failures and how to do differently」等小节；
`raw_memories.md`、`rollout_summaries/`、`memories_1.sqlite` 是更细的单会话级产物。
这些 bullet 本身就是记忆点，不需要再调模型。

```sh
cd ~/relic-vault
relic import-codex-memory --dry-run            # 只看会导入什么
relic import-codex-memory                      # 写入待检索条目
relic import-codex-memory --exclude-task-group Relic2077
```

- **只导入已提炼的句子**：解析器只接受识别到的小节里的 bullet；`rollout_summary_files`
  与 `keywords` 是指针和元数据，不生成记忆。`###` 小节若无可识别标题，会被跳过。
- **不调用任何模型**。搬运的是模型已经写好的结论，因此条目带 `codex-memory` /
  `unverified` 标签，置信度默认 0.6（低于“已验证”区间），可用 `--confidence` 调整。
- **身份来自 bullet 文本**：ID 为 `relic-codex-memory-<sha256>`，重复导入返回
  `skipped`，不产生重复条目；Codex 改写某条 bullet 后会生成新条目而非静默覆盖
  （`--update` 可改为刷新同一条目的正文）。
- **不写入他人文件**：目标路径固定为 `entries/inbox/codex-memory-*.md`，若该路径已存在
  且不属于本次导入，直接报错而不覆盖。
- **`--exclude-task-group <子串>`** 可跳过整个主题组，可重复。Codex 也会蒸馏本项目自己的
  历史，导入这类组会和知识库已有内容重复，建议显式排除。
- 与历史会话提炼不同：本命令**不写队列、不生成待审核候选**，条目直接可检索，用标签和
  置信度把关。需要审核流程时改用 `relic queue` 那条路径。
- 与 hooks、daemon、UI 轮询无关：只在手动执行时读取 `MEMORY.md`，不会自动扫描或补采。

已知但本次未处理：`relic search "MEMORY.md"` 这类含 `.` 的查询会报
`fts5: syntax error near "."`，因为 FTS5 的 `MATCH` 直接收到未经转义的原始查询
（`src/index.rs`）。这会影响检索含点号的项目名（如 `Pan.C.par`），与导入本身无关。

## 第二阶段：可配置模型命令蒸馏

现在可将草稿生成交给外部模型命令。默认仍使用模板；只有在当前设备明确配置后，
`queue work`、`watch` 和独立 `daemon` 才调用该命令。已有的自动同步沿用
[自动化文档](automation.md)，本功能不启动后台进程或改动远端。

配置示例 `distiller.json`：

```json
{
  "backend": "command",
  "executable": "/absolute/path/to/your-model-adapter",
  "args": [],
  "timeout_seconds": 30
}
```

```sh
cd ~/relic-vault
relic distiller configure /path/to/distiller.json
relic distiller status
relic queue work --capture-id <capture-UUID>
relic queue get <capture-UUID>
# 对已有、尚未作出审核决定的模板候选重新蒸馏：
relic queue redistill <capture-UUID>
relic queue work --capture-id <capture-UUID>
# 恢复模板模式（不改写已生成草稿）：
relic distiller disable
```

配置保存在 **本机** `.relic/automation/distillation.json`，不通过 Git 同步。
不会读取共享 `.relic/config.yaml` 中的可执行命令。命令使用参数数组启动，不经 shell
拼接；配置应指向可信的绝对路径。认证信息由适配器从环境或其自身凭据管理器读取，
不要放进参数。Relic 不附带模型或模型账号，需提供符合下述协议的适配器。命令自身
可以调用本地模型或获授权的模型服务；启用前确认该适配器的数据流向。

### 适配器协议 v1

标准输入是一份 UTF-8 JSON：

- `version: 1`；
- `instructions`：蒸馏目标、输出结构，以及将所有来源文本视为数据的约束；
- `source`：完整来源事件，含 `input.context/action/outcome/evidence`；
- `related_knowledge`：最多 10 个相关条目，每个包含 `id/title/content`，正文最多
  2,000 字符。当前通过相同标题或标签筛选；并非语义检索。

适配器将这些内容交给模型，并向标准输出写入**一个 JSON 对象**，不要附带代码块、
进度信息或额外说明：

```json
{
  "knowledge": {
    "title": "索引丢失时重建搜索",
    "content": "当 Markdown 完整而本地索引丢失时，从 Markdown 重建索引。适用范围和验证依据应保留。",
    "kind": "lesson",
    "confidence": 0.7,
    "tags": ["search", "recovery"]
  },
  "recommendation": "new",
  "rationale": "具有可复用价值，但引用的测试仍需审核者核对。",
  "evidence_indices": [0],
  "related_entry_ids": []
}
```

`recommendation` 为 `new`、`update` 或 `skip`，只是建议。`update` 必须引用至少一个
传入的相关条目 ID。`evidence_indices` 为来源 `evidence` 数组的零基索引；没有证据时
必须为空。超出输入范围的引用、无效类型或置信度、空理由、非 JSON 输出均报错。
即使建议跳过，也要提供一份简短草稿与理由供复核，不自动拒绝事件。验证引用存在
不等于验证证据支持模型结论。

### 恢复与执行边界

- 模型处理期间释放采集锁，新 capture 不必等待推理；另一个处理器返回 `busy: true`。
- 单次请求最多 512 KiB，响应最多 64 KiB，命令超时 1–60 秒。worker 每批在达到
  30 秒后不再开始下一事件；当前事件最多执行到其配置的超时。
- 使用临时工作目录和匿名输入输出文件，防止管道堵塞；不把 stderr 或无效模型响应
  复制到队列错误。超时或异常会终止直接子进程。适配器应同步执行，不能自行后台化；
  此版本不保证终止适配器创建的全部后代进程。
- 出错进入 `failed`，修复配置后 `queue retry`；没有自动悄悄降级为模板。重新蒸馏失败
  时保留旧草稿，但它仍处于失败状态，不能跳过处理直接收录。
- 模型结果保存为 `needs_review`，附加 `distillation` 建议、理由和引用。CLI/MCP 能查看
  全部信息，UI 展示建议和理由。接受仍使用已有 review 接口，不自动改写相关知识。
- `relic_redistill_capture` 可通过 MCP 将未审核候选重新排队，再调用
  `relic_process_captures` 处理。已收录、已拒绝或已持久化审核决定的候选不能重蒸馏。

这是模型适配协议和可靠执行机制，不是已部署的模型服务。仓库测试使用受控的命令
适配器，覆盖请求上下文、失败重试、超时、输出上限、引用校验和采集并发；不代表
某个真实模型的知识提炼质量已通过评测。

`queue work --capture-id UUID` 或 MCP `relic_process_captures` 的 `capture_id` 参数仅处理指定事件，不触碰其他待办；未指定时按批处理。
