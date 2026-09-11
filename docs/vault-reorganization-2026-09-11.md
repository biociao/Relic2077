# 记忆库整理报告 · 2026-09-11

一次针对 `~/relic-vault`（Relic 长期记忆库）的组织整理：诊断、改动、验证、回滚。
本次把「一头沉」的库拆成**主脑 / 前意识**两层，并让标签、标题、图谱、分析四个层次重新可用。

- 整理前状态：`/tmp/relic-work/vault-backup-20260911-133610.tar.gz`（整理前完整快照）
- 变更日志：`/tmp/relic-work/tidy-applied.json`
- 工具：`tools/relic-tidy.py`、`tools/relic-relate.py`、`tools/relic-distill.py`

## 一、结果一览

| 指标 | 整理前 | 整理后 |
| --- | ---: | ---: |
| 条目数 | 203（全部 active） | **355**（352 记忆 + 3 reflection） |
| 状态分布 | active 203 | **active 22 / fading 331 / archived 1 / superseded 1** |
| 标题长度 最大 / 均值 | 236 / 183 字符 | **137 / 64 字符**（>120 字符：180 → 6） |
| 标题是「文档标题拼接」 | 182 条 | 0 条 |
| 悬空伪链接 | 182 条 | **0 条** |
| 主题标签（裸词） | 69 个混用，51 个只用一次 | **138 个**，每个 ≥2 次或属策展词表 |
| 作用域标签（带前缀） | 0 | 62 个 / 1176 次使用 |
| 置信度分布 | **182 条全为 0.6**（共 6 个取值） | **0.3–1.0 共 21 个取值** |
| `relic analyze` 假矛盾 | **1207 条** | **18 条**（全部集中在 `mNGS` 这一个宽泛领域词内） |
| pattern 候选 | 只有 1 个 183 成员的垃圾分组 | **112 个**（`Treg` 5、`PUMCH`/`DESeq2`/`NMPA` 各 3 …） |
| 图谱边 | 1041（几乎全是语义近邻，显式边 1 条） | **2067**（link 26 / supersedes 2 / tag 105 / corroborates 15 / contradicts 14 / semantic 1905），孤立点 0 |
| reflection | 0（三个目录都是空的） | daily / weekly / monthly 各 1 篇 |
| 待审队列 | 181 条原始转录（distiller=template） | **150 收 / 31 拒 / 0 待处理**，全部经模型提炼（`command-v1`） |
| distiller | `template` | **`command`**（约 8–14 s/条） |

## 二、诊断：六个真问题

1. **91% 的内容是一次批量导入。** 182/203 条来自 `codex-memory`（Codex 自身记忆产品的导入），
   全部 confidence 0.6、全部 tag `codex-memory`+`unverified`、body 都带 `**Applies to**: cwd=<绝对路径>`。
   它们是**任务作用域的项目笔记**（17 个项目：gi02-jitc 44 条、isowast 23、pan-c-par 20、rhb 15、gi03 13、hwb 12 …），
   却和 19 条真正手工沉淀的决策（0.85–1.0）混在同一层，检索时被淹没。
2. **标签体系失效并污染分析层。** 最大两个 tag 是 provenance（`codex-memory` 183、`unverified` 182），
   而 `detect_contradictions` 把**所有** tag 当 subject 且无规模保护 → 183 条两两配对 → **1207 条假矛盾**。
3. **标题不可用。** 平均 183 字符，180 条超 120，本质是把「任务组 · 章节 · 正文前 80 字」拼进标题。
4. **图谱空转。** 184 条 link 里 182 条是 `codex-memory:codex:<组名>` 的**悬空伪链**（指向不存在的条目），
   真正条目间连边只有 2 条；`supersedes` 全空，版本演进只能靠时间猜。
5. **置信度无区分度。** 182 条同为 0.6，且 `last_verified` 全是导入日 → 衰减维度信息量为零，
   `fading_threshold`（0.3）在当前参数下永远不会触发。
6. **181 条待审不是"积压"，是"没有提炼器"。** `distiller status = template`，全部 `distillation: null`，
   标题清一色「历史会话 · 原始提问前 80 字」，body 是原始对话转录；直接批量 accept 会把体积问题放大一倍。

> 更正：本次会话早期观察到 `relic doctor` 报 `attempt to write a readonly database`。
> 那是**当前会话的文件沙箱**造成的，不是库的缺陷——放宽写权限后 `doctor` 输出
> `Vault healthy: 203 valid entries; index rebuilt`。

## 三、做了什么

### Step 1–3 · 数据整理（`tools/relic-tidy.py`，196 处改动）

- **命名空间约定**：作用域一律带前缀，主体一律裸词。
  - `codex-memory` → `src:codex-memory`；`unverified` 直接删除（信息量为零，由 confidence 表达）
  - 新增 `project:<slug>`（17 个）、`section:pref|failure|fact`（按 Codex 章节映射）、`pool:preconscious`
  - 策展层：`relic2077` → `relic`（合并同义 tag）；`codex-memory` 作为**主题**被重命名为 `codex-memory-import`，避免与 provenance 同名
- **标题重写**为 `<项目> · <章节>：<≤64 字断言>`，原来的「任务组 · 章节」移入 body 首行引用块，可追溯；
  同时清理 182 条悬空伪链。
- **置信度分档**：正文含路径 / 带单位数值 / commit 哈希 → 0.65（80 条）；纯断言 → 0.5（102 条）。
  `created`、`last_verified` 保持原样（本次没有任何"验证"发生，不伪造验证时间）。
- **分层**：182 条批量条目 `status: fading` + `pool:preconscious`（前意识池：仍可检索、可召回，但不占主脑）；
  占位空壳 `第一条知识` → `archived`。
- 脚本带 **YAML 往返校验**：改写后的文件必须解析回预期元数据，否则拒写；仅格式差异不写（避免无意义 churn）。

### Step 4 · 关系层（`tools/relic-relate.py`，24 条链接 + 1 对 supersede）

- 真实边：图谱/UI 投影层、捕获→提炼→导入管线、宿主集成、版本里程碑链，每条边在脚本里都写明**为什么存在**。
- 唯一一对语义明确的取代：`relic-20260901-00a43b`（写明「SSE/sessions/OAuth 留待将来」）
  → `relic-20260902-ef49cd`（次日实现了这三项）。
- 补齐 reflection：daily / weekly / monthly。

### 代码改动 · 让约定成为真的约定

数据改完后假矛盾反而升到 1425 条——因为 `analysis.rs` 仍把 `pool:`/`project:` 当主题。约定必须是代码里的规则：

- `src/entry.rs`：新增 `is_subject_tag()` / `subject_tags()`——**带命名空间的 tag 是作用域，不是主题**。
- `src/analysis.rs`：`detect_contradictions` 与 `extract_patterns` 只看主体 tag。
- `src/graph.rs`：tag 边同样只看主体 tag。
- `tests/analysis.rs`：新增 `namespaced_scope_tags_never_produce_contradictions_or_patterns`，
  同时断言「同一批数据带上真实主体 tag 后**应该**产生矛盾与 pattern」——防止把检测能力一起关掉。

验证：`cargo test` 19 个测试目标全绿，`cargo fmt --check` 干净，`cargo clippy --all-targets` 0 warning。

### Step 0 · distiller（`tools/relic-distill.py`）

Relic 自带蒸馏契约（stdin 一个请求 JSON → stdout 一个输出 JSON），但设备上没有可用的模型通道：
`codex exec` 一次 106 s（websocket 重连）、`claude -p` 超时、dcs CLI 无 llm 子命令。
**可用的快速通道是 DSH 自身的 headless profile：`dsh --profile headless "{prompt}"`，约 2 s 启动。**

脚本按优先级选通道，并**永远返回合法 JSON**（提炼器崩溃会让队列卡死，比朴素草稿更糟）：

1. `RELIC_DISTILL_CMD`（`{prompt}` 占位 → 作为单个 argv 传入，或省略则走 stdin）
2. `RELIC_DISTILL_BASE_URL`（OpenAI 兼容端点，`urllib` 直连，无需 curl）
3. 默认 `dsh --profile headless {prompt}`
4. 无模型时的启发式兜底：抽取带偏好/证据标记的句子，仍按同一套标签约定出草稿

它还会**归一化任何模型输出**到契约之内：kind 白名单、confidence 夹取、以及**强制补上
`src:codex-history` / `project:<slug>` / `pool:preconscious`**——模型可以提议主体 tag，但作用域 tag 由约定决定。

实测一条真实捕获：7.4 s，产出标题「复现历史测序论文：原始 capillary reads 需从 Trace Archive→SRA 迁移数据获取」，
正文含四个 run 的 reads 数与「180,713 ≠ 论文 103,462/76.2 Mb」的口径区分，`backend: command-v1`。

### 规范落盘

- `.relic/taxonomy.md`：从默认占位（ai-engineering/career/…）改写为真实的主体词表 + 作用域前缀说明。
- `AGENTS.md`（vault 内）：加入标签约定与"前意识池"规则——下一个写库的 agent 会读到它。

## 四、待审队列（181 条）

179 条重新入队后用新 distiller 逐条提炼（`queue work` 每次有 30 s 墙钟预算，故用驱动循环反复调用；
52 轮、约 30 分钟、0 失败）。随后按**写下来的政策**批量审核（`tools/relic-triage.py`）：

- **拒 31 条**：提炼器自己判定 `skip`（无实质轮次、无可持久信息），或草稿过薄（正文 <200 字符）
- **收 150 条**：逐字采用提炼草稿，随后**统一降级进前意识池**（`fading` + `pool:preconscious`）——
  机器提炼、未经核验的记忆正是这个池子的定义；晋升是显式动作（`relic update <id> --status active`）

产出示例（最高置信度 0.85）：

> **投标/设备参数修订：★必选项即使缺公开证据也不得删除，应保留原文并加批注**
> 场景 → 错误做法（整条删除 ★条款，用户纠正"带星号的你也敢删啊？那是必选项"）→ 正确做法四步 →
> 适用性。标签：`投标参数`/`招标文件`/`必选项`/`证据核验` + `src:codex-history`/`project:…`

### 收尾时发现并修掉的一个回归

提炼器很会命名，但**不知道库里已有什么词**：149 条新条目一口气造出 673 个一次性主题标签——
这正是旧问题的镜像（过去是"一个标签盖住 183 条"，现在是"每条一个新标签"）。两步修掉：

1. **根因**：`tools/relic-distill.py` 现在会把库中在用的主题词表（≥2 次使用，116 个词）注入 prompt，
   要求"能复用就复用，确实没有才新建"。
2. **存量**：`tools/relic-vocabulary.py` 做受控词表收敛——项目名降为作用域、大小写归并、同义词合并
   （`benchmark-evaluation`→`benchmark`），**且只对机器产生的条目（`src:codex*`）执行频次裁剪**。
   741 → 138 个主题词。

> **边界教训**：第一次跑收敛时误伤了手工策展层（`relic-0-5-analysis` 丢了 `analysis`/`contradiction`/`pattern`，
> `topic sphere` 丢了 `canvas`）。频次规则是给机器写的：人工写下的标签是刻意的，不该被统计规律删掉。
> 已修正规则并从整理前快照恢复了 19 条手工条目的标签。

## 五、副作用与已知取舍

1. **活动日历会显示 ~200 条集中在整理日**：UI 的活动图按条目「最近 updated 日期」统计，整理会移动这一天。
   这是已知且被记录在案的行为（见库内 `relic-ui-embedded-local-dashboard` 条目），不是数据错乱。
2. **brain 横幅现在把 331 条前意识条目算作 `needs_review`**：其判定规则是「fading 或 active 低于阈值」。
   语义上说得通（它们确实未经复核），但如果你希望两者分开显示，需要改 UI 口径。
3. **残留 18 条「矛盾」**：全部落在 `mNGS` 这一个宽泛领域词内（19 条条目共享它）。
   极性检测器把"领域词"当"论断主题"，仍会误配。真要清零，需要区分 domain tag 与 claim subject，
   或给 `detect_contradictions` 加一个类似 `graph.rs` 中 `MAX_TAG_GROUP_FOR_PAIRS` 的规模保护。
   18 条是"一眼能读完"的量级，故本次不再改动分析器。
4. **一次性标签被丢弃 592 个**（机器条目）——词仍在正文里，全文检索仍可命中；变化的是"主题"这一层恢复成层。
5. **文件名未改**：仍是从旧标题派生的 `codex-memory-<hash>.md` / `capture-<uuid>.md`。UI 显示 title，
   重命名风险大于收益；如需可按 `id` 重建文件名。
6. **另一个会话正在同一棵树上工作**：`src/graph.rs`、`ui/`、`src/cli.rs` 等 40 余项未提交改动并非本次所写；
   整理的代码改动也未提交，回滚请只还原片段（见第六节）。

## 六、回滚

数据（全量）：

```bash
rm -rf ~/relic-vault && tar xzf /tmp/relic-work/vault-backup-20260911-133610.tar.gz -C ~
cd ~/relic-vault && relic reindex
```

单条复原：从 tar 中取出对应文件覆盖，或手改回 tags/status/title 后 `relic reindex`。

代码：本次改动**未提交**，且工作树里还有另一会话未提交的 WIP（`src/graph.rs`、`ui/`、`src/cli.rs` 等 40 余项）。
因此**不要整文件 `git checkout`**，只还原本次引入的片段：

- `src/entry.rs` — 删除 `TAG_NAMESPACE_SEPARATOR` / `is_subject_tag` / `subject_tags`
- `src/analysis.rs` — 两处 `subject_tags(&entry.meta.tags)` 改回 `&entry.meta.tags`，并去掉 import
- `src/graph.rs` — `collect_tag_edges` 里改回 `entry.meta.tags.iter()`，并去掉 import
- `tests/analysis.rs` — 删除 `namespaced_scope_tags_never_produce_contradictions_or_patterns`

工具脚本：`tools/relic-tidy.py`、`tools/relic-relate.py`、`tools/relic-vocabulary.py`、
`tools/relic-triage.py`、`tools/relic-distill.py`（新增，可直接删除）。
distiller 配置：`relic distiller disable` 回到 template 后端。

## 七、后续建议

1. **promotion 流程**：定期从前意识池挑条目——`relic list --status fading --tags project:gi03`
   → 核对 → `relic update <id> --status active --confidence <更高的值>`。这样"主脑"是被主动浇灌的。
2. **`relic watch --once` 纳入日常**：索引 + 自动 reflect。
3. **import 侧约定化**：`import-codex-memory` 目前把 `cwd=` 留在 body 里；后续可加 `--project-tag` 与标题模板，
   让下一次导入天生符合约定（本次是用 `tools/relic-tidy.py` 事后归一的）。
4. **别再用一个 tag 标一整批导入**：这是 1207 条假矛盾的唯一根因。批次的身份属于 `source_agents` 与 `src:` 作用域。
