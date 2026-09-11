# 记忆自动化钩子

现已实现三条链路：

1. **任务前检索**：Codex 的 `UserPromptSubmit`、DSH 的 `agent/pre-step` 检索活跃记忆，把有限长度的参考片段注入当前任务。
2. **轮次结束采集**：保存该轮用户提示与最终回复（最多 8,000 / 24,000 字符），生成未验证的来源事件。后台每 5 秒准备候选，人工或 Agent 审核后才进入正式记忆。
3. **后台同步**：`sync.mode: auto` 时，独立 worker 检查知识库文件变更，稳定 15 秒后同步，连续修改最多等待 60 秒；无本地变化时每 60 秒拉取。网络失败指数退避，Git 冲突暂停。

默认生成模板草稿；可按 [模型蒸馏配置](memory-pipeline.md#第二阶段可配置模型命令蒸馏) 接入本机模型命令。不会自动核实回复中的事实，也不会自动采纳候选。自动采集的来源事件会随知识库 Git 同步，因此仅为希望保存并同步任务提示和回复的项目启用。不会读取完整会话文件、思考内容或工具原始日志。关闭 Agent 不影响已落盘的来源与候选；退出独立 worker 则暂停处理，下次启动恢复。

## 构建

```sh
cargo build --release
```

将生成的 `relic`（Windows 为 `relic.exe`）放在固定位置，后续安装钩子时会记录当前可执行文件的绝对路径。UI 与 worker 应使用同一版本。

## Codex

为单个项目启用，明确指定项目 hooks.json 与知识库路径：

```sh
relic install-hooks --vault /absolute/vault --config /absolute/project/.codex/hooks.json --dry-run
relic install-hooks --vault /absolute/vault --config /absolute/project/.codex/hooks.json
```

也可将 `--config` 指向 `~/.codex/hooks.json`，全局启用时会采集该配置覆盖的所有任务。安装器合并 `UserPromptSubmit` 和 `Stop` 命令钩子、保留已有配置、创建备份；重复安装同一命令不重复追加。更换可执行文件位置时，请删除旧 Relic 命令后重新安装，避免两个版本同时触发。

Codex 对新增或变更的命令钩子有宿主信任审核，安装文件后仍需要在 Codex 内完成审核，再开始新任务。Relic 不会绕过此流程。官方接口参考：[Codex Hooks](https://learn.chatgpt.com/docs/hooks)。

`relic hook --host codex --vault /absolute/vault` 从标准输入读取不超过 1 MiB 的 JSON，标准输出只写宿主协议 JSON。正常 Stop 返回 `{}`，不阻止 Agent 停止、不强制续轮。优先以宿主 `turn_id` 去重；若未提供，则使用该会话最近一次 UserPromptSubmit 持久化的轮次 ID。没有轮次标识且没有先前提示时会报告错误，不制造不可靠的去重键。

## DSH

DSH 0.1.2-rc.1 的 Codex 兼容桥在 Stop 中传入空回复，因此使用仓库内原生适配器。适配器以参数数组启动 Relic，无 shell 插值；从 DSH 当前会话的公开事件快照读取本轮最后一个完整 `assistant/message`。

把下列插件项合并到 DSH 加载的 `cordis.patch.yml`（如 `$DSH_HOME/cordis.patch.yml`），路径改为实际绝对路径，然后重启 DSH：

```yaml
- name: /absolute/Relic2077/integrations/dsh/relic.mjs
  config:
    executable: /absolute/Relic2077/target/release/relic
    vault: /absolute/vault
```

Windows 路径可用正斜线，例如 `C:/Relic/relic.exe`。该适配器只依赖 Node 内置模块，无额外 npm 依赖。不要同时为同一知识库安装 DSH 兼容桥的 Relic Stop 钩子。其他用户钩子可以保留。

DSH `agent/pre-step` 是等待完成的调用，能在模型请求前加入上下文；`agent/turn-stopping` 只落盘，不 steer。宿主强制终止进程时，未收到的 Stop 无法补采；已经成功落盘的事件可以恢复处理。原生适配器的接口依据本地安装版本核对，升级 DSH 时请运行下方适配器测试并确认接口兼容。

## 独立 worker 与同步配置

```sh
relic daemon --vault /absolute/vault
```

此命令前台持续运行，与 Agent 进程独立。macOS 可由 launchd 管理，Linux 可由 systemd 用户服务管理，Windows 可由任务计划程序在登录时启动；三者的程序/参数分别指向固定的 Relic 可执行文件与 `daemon --vault /absolute/vault`。当前命令不会自动安装系统服务。也可先在独立终端运行，使用 Ctrl+C 停止。

单次处理、诊断或重试：

```sh
relic daemon --vault /absolute/vault --once
relic daemon --vault /absolute/vault --once --retry
```

只处理队列时保留 `sync.mode: manual`。启用自动 Git 同步需要在知识库 `.relic/config.yaml` 明确设置：

```yaml
sync:
  mode: auto
  remotes:
    - name: origin
      url: git@your-host:your-account/your-vault.git
```

自动 worker 使用第一个配置远端和当前分支。知识库必须是独立 Git 仓库；不允许意外提交其父项目。首次使用需配置 Git 提交身份、非交互凭据及远端；远端已存在但 URL 不一致时会报错，不改写远端。不会创建托管平台仓库、购买服务或自动配置凭据。

同步会提交知识库内未被 Git 忽略的文件，包括正式记忆、来源事件及配置，与 `relic sync` 的范围一致。`.relic/queue` 与 `.relic/automation` 是本地状态，不上传。不要手动将这些目录加入 Git 跟踪。自动同步不要求弹窗或密码输入，每个外部 Git 操作限制约 30 秒，失败后保留本地提交重试。首次失败等待 30 秒，指数退避上限 15 分钟。提交不启动 GPG 交互签名。

与手动同步的区别：自动同步发生冲突后保留 Git 的冲突文件和双方提交，不选择 ours/theirs。先在知识库内检查、解决并提交冲突，再在 UI 点击“重试同步”或使用 `--once --retry`。未解决时重试会再次暂停。

## UI 与恢复

打开 `relic ui --vault /absolute/vault` 的“自动化与同步”页面，可查看：

- worker 最近活动、同步模式、上次成功时间及错误；“最近活跃”表示 120 秒内有 worker 活动，不保证该进程仍存活。
- 最近 50 条候选，展开原文后填写依据采纳或拒绝；需要修改候选时可通过 CLI/MCP 提交带 `knowledge` 的审核。
- 处理待办队列、重试失败候选；“重试同步”只安排下一次尝试，需要独立 worker 运行。
- 最近 50 个钩子事件与错误，不将提示和回复重复写入事件列表。

候选审核沿用 [memory-pipeline.md](memory-pipeline.md) 的持久化契约；`.relic/queue` 包含未采纳草稿与审核决定，需要单独备份。`.relic/automation` 保存会话轮次关联、事件列表、重试与暂停状态；删除会失去去重的上下文和暂停状态，不要在运行中删除。锁文件不可删除；进程退出时 OS 自动释放锁。

尚未实现：PreCompact 采集、内置模型服务、自动证据核查、跨设备同步候选审核决定、系统服务的一键安装。当前仅在 macOS 上运行验证，Windows/Linux 使用可移植实现，仍需对应平台实际验证。

## 验证

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo build
node --test integrations/dsh/relic.test.mjs
```

覆盖：重复 Stop 去重、未知/空回复、上下文注入、候选审核隔离、合并配置、真实本地 bare Git 远端同步与冲突暂停、UI 请求保护，以及 DSH 原生适配器调用真实 Relic 二进制。测试使用临时知识库，不推送用户数据。

## 接入与启用入口

自动化页面现提供项目路径、DSH 数据目录和三个操作：安装 Codex 项目钩子、安装 DSH 原生插件、启动/停止独立后台。也可使用：

```sh
relic setup --vault /absolute/vault --project /absolute/project --host codex
relic setup --vault /absolute/vault --project /absolute/project --host dsh --dsh-home /absolute/dsh-home
relic setup --vault /absolute/vault --project /absolute/project --host worker
relic setup --vault /absolute/vault --project /absolute/project --host stop
```

DSH 插件复制到知识库的本地 automation 目录，配置带 project 限定，仅采集指定项目及其子目录。已有插件配置保留，安装前创建备份。当前入口管理一个项目与知识库的接入关系；更换项目会更新 DSH 的范围，但不会自动删除旧项目 Codex 钩子。

界面分别显示“未安装”“已安装 · 等待首次触发”“已收到事件”和错误状态。安装状态按磁盘配置核验，触发状态需要匹配项目的真实钩子事件；不会为填充空列表制造采集记录。后台运行状态通过进程持有的 OS 文件锁核验，单次维护不冒充常驻后台。启动操作创建独立进程，不安装开机自启服务；系统重启后需要重新启动。停止请求将在当前处理结束后生效。


## 手动提炼历史会话

UI 左侧的「历史会话提炼」与实时 hooks 相互独立。打开页面只读取默认路径；
只有点击「扫描历史会话」才读取本地历史，只有勾选会话并点击「提炼所选会话」
才提交来源并准备候选。daemon、hooks、页面刷新和轮询均不扫描或补采历史。

支持 Codex rollout JSONL 和 DSH JSONL / session.jsonl.zstd。默认目录分别为
`$CODEX_HOME/sessions`（或 `~/.codex/sessions`）和 `$DSH_HOME/sessions`
（或 `~/.dsh/sessions`）；可手动选择其他会话目录，例如 Codex 的 archived_sessions。
不会自动连接远端主机。每次扫描最多 1,000 个会话文件，单文件解压后不超过 64 MiB。
来源只提取完成轮次的用户文本和最终回复，不保存思考、工具日志或系统提示。
各轮文本沿用 hooks 的 8,000 / 24,000 字符上限；合并内容超过 180 KB 时不导入。

会话 ID 与 Agent 来源用于区分身份，已有采集（包括 pending、failed、rejected）
或正式记忆关联的会话不可再次导入。扫描后文件改变会拒绝导入。会话级检查与
实时 capture 共用文件锁；同一快照重复提交返回原候选。历史来源保留覆盖的轮次 ID
与文本对哈希，迟到的实时 Stop 不重复保存已覆盖轮次，后续新轮次仍可采集。

旧版主动写入可能没有 Session 来源字段：若历史中检测到 Relic 写入调用，界面会
标记「旧记忆关联需核对」，不会将它误称为已成功收录，也不会直接再次导入。
分支会话可能带有父会话历史，同样暂不导入。无来源关联、也没有可识别写入调用的
旧知识无法保证自动识别；最终仍须核对候选及相关知识，不能把会话级去重说成语义去重。

每个手选会话生成一个带 `manual-history` 来源的候选；自动采集为 `automatic-capture`。
审核列表显示来源、项目及会话 ID。未配置模型时明确显示生成的是原文模板候选；
配置模型命令后才进行模型提炼。两种候选均需要审核，手动历史操作不会自动收录知识。
失败候选在「自动化与同步 → 采集与审核」中重试，不重新导入整个会话。

历史列表按项目分组，支持「全选当前筛选中的可用会话」「选中本组」「清空全部选择」
以及「排除整个项目 / 恢复项目」。排除会清除该项目所有已选会话，即使部分会话被
搜索筛选隐藏；后续全选继续跳过排除项目。排除仅属于当前页面的手动批次，不修改
全局实时 hooks，不写入持久配置。切换来源目录或重载页面后重新选择。
未配置模型时按钮显示「生成所选模板候选」；配置模型后显示「用模型提炼所选会话」。
