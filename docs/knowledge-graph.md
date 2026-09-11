# 知识图谱与矢量关系网

本文说明 Relic2077 的知识图谱架构：它如何把录入的记忆变成一张**带类型与证据的关系网**，如何在本地、无模型、无网络的条件下建立**逻辑矢量关系**，以及为什么它是可删除、可重建、可审计的。

相关代码：

| 模块 | 职责 |
| --- | --- |
| `src/text.rs` | 确定性分词器（向量层的输入契约） |
| `src/embedding.rs` | 矢量层：哈希 TF-IDF 嵌入、余弦近邻、可解释词表 |
| `src/graph.rs` | 逻辑层：带类型/推导方式/证据的边、遍历与导出 |
| `src/search.rs` | 检索层：关键词 / 矢量 / 混合（RRF 融合） |
| `src/analysis.rs` | 结论极性判定（冲突与互证的信号源） |
| `src/vault.rs` | 指纹、缓存、按需重建 |

---

## 1. 设计目标与约束

Relic 的既有契约决定了架构的形态：

1. **Markdown 是唯一事实来源。** 图谱与向量都是**派生产物**，可随时删除重建，永不反向写回 Markdown。
2. **本地优先，不依赖模型与网络。** 因此不能调用托管的 embedding API。
3. **确定性。** 同一份 Markdown 必须产生完全相同的图谱，否则无法测试、无法版本化。
4. **"真实有效"而非装饰。** 图谱必须真正改善检索与推理，并且每个推断出的关系都要能被读者检验。

前三条约束排除了"调用大模型生成 embedding 并建向量库"的做法；第四条排除了"只画一张好看的图"的做法。剩下的选择是：**用真实的稀疏信息检索几何（哈希 TF-IDF + 余弦）作为矢量层，用带证据的类型化边作为逻辑层。**

## 2. 两个投影：矢量空间与关系网

同一批记忆有两个互补的视图：

- **矢量层（连续、语义）**：每条记忆是 `D` 维空间中一个 L2 归一化的稀疏向量。"像不像"由余弦相似度回答。
- **逻辑层（离散、命名）**：节点是记忆，边是**有名字的关系**。"为什么相关/冲突/被替代"由边的类型与证据回答。

两层由一条明确规则缝合：**余弦 ≥ 阈值 ⇒ 生成一条 `semantic` 边**，并把支撑该相似度的词表写进边的证据里。于是矢量不再是一个只输出数字的黑箱，而是一个能说明理由的关系。

### 2.1 矢量层如何工作

1. `text::tokenize` 把记忆切成 token：标题重复两次、标签拼入正文，因为它们本身就是浓缩的摘要；正文各一次。
2. 亚线性词频 `1 + ln(tf)` 乘以 BM25 风格的逆文档频率 `ln(1 + (N - df + 0.5) / (df + 0.5))`（恒为正，避免"共享词反而降低相似度"）。
3. `text::hash_token` 把 token 映射到**带符号**的维度（FNV-1a 定位 + SplitMix64 决定符号）。符号很关键：哈希碰撞时相互抵消而不是相互叠加，维度较少时无关文本的余弦仍接近 0。
4. 向量做 L2 归一化 ⇒ 内积即余弦。
5. **可解释性**：语料同时记录每个被占用维度上最多 3 个代表性 token。这就是"为什么这两条记忆相似"能回答出**词语**而不只是数字的原因。

分词规则中的两个实际决定：

- **英文复数折叠**（`chunks → chunk`）。没有它，`chunks` 与 `chunk` 落在不同维度，相似度完全丢失。实测把最弱的相关记忆对从 0.183 提升到 0.289。
- **CJK 字符级 unigram + bigram**。中日韩文没有空格词边界，按整段匹配会漏检。字符 n-gram 让"逻辑矢量关系"能命中"逻辑矢量关系网"。

### 2.2 逻辑层有哪些边

每条边记录三件事：**类型**（`kind`）、**推导方式**（`derivation`）、**证据**（`evidence`）。

| 类型 | 推导方式 | 生成规则 | 权重 |
| --- | --- | --- | --- |
| `link` | explicit | front matter 中的 `links` | 1.0 |
| `supersedes` | explicit | `supersedes` / `superseded_by`（版本历史） | 1.0 |
| `tag` | statistical | 主题标签的 Jaccard 重叠 ≥ `tag_min_jaccard` | Jaccard |
| `semantic` | statistical | 向量近邻，余弦 ≥ `semantic_min_similarity`，每节点取 top-k | 余弦 |
| `contradicts` | logical | 共享主题 + **方向相反的明确结论** | 0.8 |
| `corroborates` | logical | 共享主题 + **方向相同的明确结论** + 余弦 ≥ 阈值 | 余弦 |

几点刻意的取舍：

- **来源 Agent 不是一种边。** 单 Agent 的知识库里所有记忆共享同一个来源，若把它作为边，整张图会退化成一个团块，反而摧毁了图要展示的结构。来源信息保留为节点属性与统计维度。
- **显式关系永不被裁剪。** `link`、`supersedes`、`contradicts` 是"事实性"的，不参与每节点边数预算。
- **推断关系有预算。** 超出 `max_edges_per_node` 的推断边按权重从弱到强裁剪，裁剪数量写入 `Graph::notes`。
- **对称边规范化。** `tag`/`semantic`/`contradicts`/`corroborates` 以字典序较小端点为起点，避免同一对记忆出现两条镜像边。
- **有界即声明。** 主题组超过 2048 条时不做两两枚举，并在 `notes` 里说明，而不是静默改变图形状。

### 2.3 "结论冲突"为什么必须严格

冲突边是全图价值最高、也最危险的边：一条假的冲突会误导读者，并污染所有基于它的反思文档。

最初的实现用**子串**匹配极性词表，在真实文本上产生了假阳性：

- `validate` 包含 `valid` ⇒ "validate the split against the corpus" 被算作正面主张；
- `against the corpus` 被算作负面主张；
- `avoids mid sentence cuts` 被算作负面主张。

结果两条**互相印证**的分块策略记忆被报告为"结论冲突"。

修复分三步：

1. **按词匹配而非子串匹配**，并只接受规则屈折（`s`/`es`/`ed`/`d`/`ing`）：`fails` 匹配 `fail`，但 `validate` 不再匹配 `valid`。
2. **移除 `against`**：在技术写作中它几乎总是介词（validate against、reconcile against），却会给无立场的句子打上负面判定。
3. **要求"明确的结论"**：一侧必须比另一侧多出至少 2 个同向标记（`CLAIM_MINIMUM_STRENGTH`）才算陈述了主张。孤立的 `avoids` 不算。

这是一个有意的取舍：**宁可漏掉微妙的矛盾，也不报告错误的冲突。** 错边的代价远高于漏边。

## 3. 派生数据的生命周期

```
Markdown (唯一事实来源)
        │  指纹 = SHA-256(排序后的 id + 每条记忆全字段内容哈希)
        ├─► .relic/embeddings/store.bin   矢量层（zstd + JSON，可删除）
        └─► .relic/graph.json             关系网（人类可读 JSON，可删除）
```

- 指纹**按内容**计算，不只看 `updated` 时间戳：手工编辑若忘记更新时间戳，仍然会使缓存失效。
- `Vault::graph()` 先比对指纹，命中则只读一个小 JSON，未命中才重建。因此查询路径调用它是安全的。
- 缓存缺失、损坏、版本不符、指纹不符、tokenizer 版本不符 ⇒ `load` 返回 `None` 并重建。**对派生产物而言这些都不是错误。**
- `relic graph build --force` 先删除派生产物再重建，用来证明重建确实只读 Markdown。
- 两个产物都在 `.gitignore` 中：只版本化知识本身。

## 4. 查询接口

### 4.1 命令行

```bash
relic graph build [--force] [--json]     # 重建矢量层与关系网
relic graph stats [--min-weight W]       # 规模、连通分量、中心节点、孤立记忆
relic graph neighbors <id> [--depth N] [--min-weight W] [--kind semantic,link]
relic graph path <from> <to>             # 最强推理链
relic graph explain <from> <to>          # 两个记忆之间的全部关系及证据
relic graph similar <id> [-n N] [--min-similarity S]
relic graph export [--format dot|mermaid|json] [--min-weight W]

relic search "查询" [--mode keyword|semantic|hybrid]
```

`relic graph neighbors` 的分数是路径上边权的**乘积**，所以一串弱关系排在一条强关系之后；`--min-weight` 用来只看主干。

`relic graph path` 求的是**最宽路径**：最大化整条链中最弱的一跳。这样一条强边不会被藏在几跳强关系后面而显得可靠，`bottleneck` 就是整条链的置信度。

### 4.2 MCP 工具（供 Agent 使用）

| 工具 | 用途 |
| --- | --- |
| `relic_find_similar` | 按记忆或自由文本找矢量近邻，附相似度与支撑词表 |
| `relic_graph_neighbors` | 一个记忆的类型化邻居，可按类型/跳数/权重过滤 |
| `relic_graph_path` | 两个记忆之间的最强推理链 |
| `relic_explain_relation` | 两个记忆之间每条关系的类型、推导方式与证据 |
| `relic_graph_stats` | 关系网健康度：各类型计数、连通分量、中心与孤立记忆 |

`relic_search` 新增 `mode` 参数（默认 `keyword`，保持既有客户端行为不变）；`mode: "hybrid"` 同时使用两层。

**这些工具默认只读**（`readOnlyHint: true`）：遍历关系永远不会改动知识库。

### 4.3 混合检索

关键词层与矢量层的分数不可比（BM25 无上界，余弦在 [0,1]）。因此混合模式用 **RRF**（倒数排名融合）只融合**排名**：

```
score = Σ 1 / (60 + rank)
```

被两层同时排在前面的记忆上升，只有一层能看到的记忆仍然出现。这避免了对两个不同量纲做归一化的任意选择。

实测效果（`relic search "chunks prose tokens alpine"`）：关键词层因隐式 AND 返回 0 条，混合层给出 3 条并注明各自的余弦与支撑词表。

### 4.4 中文检索

FTS5 把连续的中文字段当作**单个 token**，因此关键词检索对中文是字面且脆弱的。两处处理：

- 关键词层：对非 ASCII 连续串附加**前缀匹配**（`"知识"*`），使查询 `知识` 能命中含 `知识图谱` 的记忆。
- 矢量层：字符 unigram + bigram 让中文语义检索真正可用（本仓库实测余弦 0.28 / 0.52）。

## 5. 阈值是量出来的，不是猜的

`GraphConfig` 的默认值来自对真实记忆的测量，并由 `tests/graph_calibration.rs` 守护：

| | 最小值 | 最大值 |
| --- | --- | --- |
| 共享主题的记忆对 | 0.289 | 0.328 |
| 无关的记忆对 | — | 0.035 |
| `semantic_min_similarity` | **0.08** | |

0.08 位于 0.035 与 0.289 之间，两侧都有 2 倍以上余量（测试会断言这个余量）。测试同时断言：共享主题的记忆对**必须**产生 semantic 边，无关记忆对**必须不**产生 semantic 或 tag 边。任何破坏这一分离的改动（分词、加权、哈希）都会在这里带着数字失败，而不是悄悄产出一张全是噪声的图。

`corroborate_min_similarity` 取 0.15：互证比"相近"断言得更多，因此门槛高于语义下限。

**语料太小时图谱会稀疏，这是正确的。** 逆文档频率需要语料才有区分度；少于 5 条记忆时语义边自然很少。构建时会把这一点写进 `notes` 说明，而不是让读者去猜。

## 6. 已知限制

- **矢量层是词袋模型。** 它能发现措辞不同但**用词重叠**的记忆，不能发现真正的同义改写（例如中英文互译）。这是"无模型、无网络"的必然代价；若要语义级嵌入，需要引入可插拔的本地 embedding 提供者，目前尚未实现。
- **冲突检测是启发式。** 它比较的是结论极性词表，不是自然语言推理。文档与输出都把它称为**复核信号**，而不是判定。
- **`slugify` 仍会丢弃非 ASCII 字符**，因此纯中文标题的文件名会是 `entry-<短id>.md`。记忆 ID 仍然唯一，数据没有损失，但文件名不可读。这是既有行为，本次未改动。
- **语义边的构建代价是 `O(Σ 倒排表长度)`**，与语料规模近似线性；主题边的代价是主题组规模的平方，超过 2048 条时不再展开并记录说明。

## 7. 复现与验证

```bash
cargo test                      # 153 项测试
cargo test --test graph_calibration -- --nocapture   # 打印阈值分离实测值
cargo clippy --all-targets
cargo fmt --check
```

在一个真实知识库上端到端验证：

```bash
cd ~/relic-vault
relic graph build --force
relic graph stats
relic graph explain <id-a> <id-b>
relic search "分块策略" --mode semantic
relic ui    # 打开「关系网络」视图
```

界面中的「关系网络」视图渲染的是**同一份推导结果**（`GET /api/graph`），不是另一套数据：每条连线都可点击查看其证据，节点大小表示关系数量，孤立记忆单独计数。为保证可读性而被省略的节点与关系会在视图中明确说明。
