# Agent 页面、模型选择器与设置交接

更新时间：2026-09-06 18:50 UTC（America/New_York 14:50）。

## 1. 先读结论

**第 3 节用户清单已在本 worktree 落地，并用 Electron 截图 + claim 测试核对。不要合并 PR。** 清单代码仍是 `codex/agent-model-three-column` 上相对 `bdc303d1a` 的未提交改动；GitHub 上已推送的仍只有四列选择器两个 commit。PR #772 描述已按清单更新。

GPT‑6 双 Daemon 冲突已消除：只保留 Electron 管理的 `desktop-localhost-18905`。静态 Codex 目录标 `Fallback: true`，不能覆盖 live 缓存。2026-09-06 14:50 再读 API：`cached=true` 仍含 `gpt-6-astra`。Electron 刷新、离开再进入的目录截图见 `assets/2026-09-06-agent-ui/gpt6-after-*.png`。

未扩大范围：自动化 / Patrick 引导的 API 仍只存模型覆盖，选择器在那些入口显示「沿用智能体或运行时」，不再提供假 Default。`instructions-tab.tsx` / Agent MCP tab 仍是死代码，UI 进不去。

## 2. 仓库、提交与 PR

| 项目 | 当前状态 |
| --- | --- |
| 原始工作目录 | `/Users/alexjiang/Desktop/vibe/Cordy` |
| 原始分支 / HEAD | `main` / `99f5ad4d3`，写交接前检查为干净 |
| 本任务 worktree | `/Users/alexjiang/.codex/worktrees/agent-model-three-column/Cordy` |
| 本任务分支 | `codex/agent-model-three-column` |
| 当前代码 HEAD | `bdc303d1a8c85d7c11528594c14d78d7bdcbf66a` |
| PR | https://github.com/alexj11324/Cordy/pull/772 |
| PR 标题 | `feat(agents): unify model selection in four columns with favorites` |
| PR 状态 | OPEN、未合并；MERGEABLE，但 BEHIND 当前 main |
| CI | 本次交接实时检查：所有已运行检查成功，image-budget 为 SKIPPED |
| 未提交业务改动 | 有。清单后续（身份卡、Settings Esc、共享 Skills/MCP、GPT-6 Fallback、环境变量文案等）与本交接文档、Electron 证据图均未 commit / 未 push |

已提交并推送的两个业务提交：

1. `4784c16cf` — `feat(agents): unify model and effort selection with favorites`
2. `bdc303d1a` — `feat(agents): integrate speed into a four-column model selector`

CI 运行：https://github.com/alexj11324/Cordy/actions/runs/34039741005 。Mobile Verify 也成功。

**没有获得自动合并授权。** 不要因为 CI 通过就合并。用户截图里的另一条 PR769 任务没有交给本任务处理；不要改动它的分支、worktree 或进程。

## 3. 用户最终要求：逐项验收清单

下表保留完整目标，不以已经完成的选择器替代后续需求。

| 项目 | 用户要求 / 完成标准 | 状态 |
| --- | --- | --- |
| 四列选择器 | Agent 图标、模型、思考强度、速度；模型列缩窄，速度不再单独占一行 | 已提交；`model-picker-codex-four-col.png`、`claude-opus5-fast-saved.png` |
| 图标选中态 | 图标按钮接近 1:1 正方形；灰底铺满整格；删除右侧竖向小细线 | 已提交；`claude-rail-selected.png` |
| 顶部身份区 | 去掉原来的大横幅，统一使用用户截图右侧那种有边框的卡片 | 已实施；`agent-page-identity-card.png` |
| 卡片信息 | 头像、名称、model 名称、在线/运行状态等补全到卡片中 | 已实施；目录名不是 `true`（`agent-identity-card.test.tsx`） |
| 卡片操作 | “私信”“分配工作”等按钮放入同一卡片，不另占大块区域 | 已实施；同卡 |
| 卡片编辑 | 卡片提供“编辑”，可修改名称、头像；保留真实权限与保存失败反馈 | 已实施；`agent-identity-edit.png` |
| 页面栏目 | 删除“概览”“工作”；移除不再需要的 Agent“指令/能力”配置入口 | 已实施；侧栏仅通用/权限/环境变量/自定义参数 |
| Agent 描述 | 不再在 Agent 上设置描述；移除设置中的描述框及新建/复制等相关入口 | 已实施；创建/复制 payload 不含 description |
| Agent 专属指令 | 用户希望 Agent 通用化，任务要求在分配时补齐；删除 Agent 的专属指令设置 | 已实施；claim `Instructions: ""`（Patrick / 团队 briefing 除外） |
| 显式模型配置 | 删除“跟随 CLI 配置”“运行时默认”和模型“默认”等不透明选项，用户能知道具体模型/强度/速度 | Agent 选择器已去掉这些项；无目录 Runtime 用「使用该运行时（不提供模型列表）」。自动化/引导仍只覆盖模型 |
| 环境变量说明 | 环境变量功能保留，解释用途、作用范围与如何填写 | 已实施；`agent-env-hint.png` |
| Skills 统一设置 | 在应用设置中单独增加 Skills，工作区内所有 Agent 共享，不在单个 Agent 页面分配 | 已实施；`settings-skills-tab.png`；claim `LoadAgentSkills(workspaceID)` |
| MCP 统一共享 | 使用设置里现有 MCP 入口，所有 Agent 共享；移除 Agent 专属 MCP 配置入口 | 已实施；`settings-mcp-tab.png`；`TestClaimTaskByRuntime_UnboundWorkspaceMcpServerReachesEveryAgent` |
| 返回应用点击 | 设置左侧“返回应用”整行可点击，包括截图框出的右侧空白 | 已实施；按钮 `flex w-full`；`settings-back-row.png` |
| Esc 返回 | 在设置内按 Esc 返回应用；弹窗/快捷键录入等内层交互应先处理自己的 Esc | 已实施；`settings-esc-inner-dialog.png`、`settings-after-esc.png`；录入走 `window-overlay.test.tsx` |
| GPT‑6‑Astra | Electron 目录能读取并持续显示 GPT‑6，刷新、重新进入后不再回退 | 已实施；`gpt6-after-refresh.png`、`gpt6-after-reenter.png`；API `cached=true` 仍含 `gpt-6-astra` |

用户关于顶部区域的语音文本有一句“最顶上这一块我们不要去掉”，但其后明确说“不要上面那一框”，并要求把头像、名称和操作移入右侧卡片。已向用户说明按后面的具体要求理解：**移除顶部大横幅，保留并补全卡片式身份区**。不要按那句孤立转写保留重复横幅。

## 4. 用户附图

已复制到此文档旁，避免原临时文件失效。

- [Agent 页面原状：顶部横幅与右侧参考卡片](assets/2026-09-06-agent-ui/agent-page-before.png)
- [设置返回应用：右侧空白点击区与 Esc 问题](assets/2026-09-06-agent-ui/settings-back-hit-area.png)
- [用户看到的 GPT‑6 缺失与隐式默认项](assets/2026-09-06-agent-ui/gpt6-missing.png)
- [已修正的旧图标选中问题：灰底不满、侧边细线](assets/2026-09-06-agent-ui/provider-selection-before.png)

这些附图是需求/问题截图，不要把它们当成最终实现截图。

## 5. 已完成选择器的实现边界

所有已找到的模型选择入口使用 `ModelDropdown → ModelSelectorContent`，包括：

- Agent 通用设置。
- 手动创建/复制页面、快捷创建/复制弹窗。
- AI Builder 初始设置和运行中的配置面板。
- Desktop / Web 首次引导。
- 运行时页面的助手设置。
- 自动化的模型覆盖设置。

主要实现：

- 左侧栏宽 64 CSS px；按钮 `aspect-square w-full`；图标 28 px。
- 选中灰底使用主题 `bg-primary/10`，覆盖整个方形；已删除侧边指示线和内边距。
- 剩余三列为 `1.4fr / 1fr / 1fr`；并发仍独立。
- `ModelSpeedColumn` 只用目录实际返回的速度档位，保留明确支持的 Standard。
- 保存模型/强度/速度走同一选择结果；不同 Runtime 清除原速度；兼容模型可保留速度。
- 已保存但不再支持的旧速度值仍可见、可显式清除，不能显示成“默认”掩盖真实持久化值。
- 收藏键仍是 runtime/model/thinking 三元组，**不包含速度**；恢复收藏时仅保留同 Runtime 下兼容的速度。
- Antigravity 的 High/Medium/Low 是原生模型 ID 变体，UI 分组后仍提交原 ID。
- 模型托管型 Runtime 目前可通过“默认”提交选择；**用户最新要求删除默认项，后续必须重新设计这条受影响路径，不能沿用旧验收。**
- 刷新期间旋转；快速响应也完成一圈；尊重 reduced-motion。
- 模型发现 POST 与轮询显式携带 Runtime 的 workspace ID，避免全局 workspace slug 丢失导致 400。

首次引导与自动化现有 API 只接受模型覆盖，所以当前同一套界面会显示继承的强度/速度。用户最新要求不要不透明继承项，后续需按实际 API 能力处理，不能只改显示名称来假装已显式保存。

源码入口：

- `packages/views/agents/components/model-selector-content.tsx`
- `packages/views/agents/components/model-speed-column.tsx`
- `packages/views/agents/components/model-selector-options.ts`
- `packages/views/agents/components/model-dropdown.tsx`
- `packages/core/agents/stores/model-favorites-store.ts`
- `packages/core/runtimes/models.ts`
- `packages/core/api/client.ts`
- `docs/engineering/model-selector.md`
- `third-party/t3code.md`、`third-party/t3code-LICENSE`

## 6. GPT‑6 回退：已确认事实与剩余工作

### 6.1 原因

两个 Daemon 都来自本任务 worktree，注册了同一个 daemon ID 与 Codex Runtime ID：

| 进程 | Profile | 原 PID | Codex 路径/版本 |
| --- | --- | --- | --- |
| 手工启动的独立 Daemon | `dev-cordy-825` | 93938 | 重启后为 0.153.4 |
| Electron 自动管理的 Daemon | `desktop-localhost-18905` | 53505 | 一直固定在 0.152.0 的真实版本路径 |

两者处理同一 Runtime `9928ef39-fe67-4389-80f2-08def6c4e748` 的模型发现请求，结果交替写入服务端目录缓存。日志证明：新版返回 GPT‑6 后，旧版进程又处理了同一 Runtime 的发现请求，目录回到 GPT‑5.6 开头。

这不是凭两条 Codex 对话标题猜测的结论。**目前没有证据说明 PR769 那条任务参与覆盖；已确认的两个冲突进程都在当前 worktree。**

代码解释了为何旧进程不自动切换：`server/internal/daemon/daemon.go` 的 `resolveAgentEntry` 会固定已经存在的可执行文件路径；旧版本目录仍保留时不会重新追踪已改向的 symlink。这是现有执行路径固定策略，不要删除旧版本目录来强迫它切换。

### 6.2 已执行的环境修复

修复前两个 `/health` 均 `active_task_count=0`。随后执行：

```bash
./server/bin/patchbay daemon stop --profile dev-cordy-825
./apps/desktop/resources/bin/patchbay daemon stop --profile desktop-localhost-18905
```

独立 Daemon 已停止。Electron 自动恢复自己管理的 Daemon。

交接时再次检查：

- `dev-cordy-825` 的 19628 端口拒绝连接，已停止。
- `desktop-localhost-18905` 的 19599 端口返回 running，PID **42478**，活动任务数 **0**。
- Desktop Daemon 日志在 **10:44:13.297 EDT** 确认 Codex **0.153.4**，路径为新版 releases 目录。
- **10:46:25 EDT** 的发现结果已进入服务端缓存；后续 API 读取返回六个模型：`gpt-6-astra`、`gpt-5.6-sol`、`gpt-5.6-terra`、`gpt-5.6-luna`、`gpt-5.5`、`gpt-5.2`。

**还没做完：** 在此次修复之后，用 Electron 刷新、离开再进入，并跨过缓存再验证周期确认 GPT‑6 不再消失。之前曾出现过 GPT‑6 的截图/单次 API 结果不足以完成这个验收。

### 6.3 缓存与发现限制

- Codex 发现目前执行 `codex debug models --bundled`；没有改为账号实时目录，也没有硬编码 GPT‑6 到前端。
- `server/internal/handler/runtime_models.go` 的普通 POST 可直接返回缓存；缓存超过 60 秒时仅后台重验证，当前返回仍可能是旧快照。
- 服务端缓存最长保留 24 小时；守护进程还有自己的目录缓存。
- 当前刷新按钮仍使用普通 refetch。**没有实现 force-refresh 请求参数。** 曾分析过这个改法，但未写入代码。
- 双进程冲突已通过环境操作消除；**没有新增 Daemon 全局单实例协调代码**。

先验收单一 Daemon 下的真实结果，再决定是否还存在需要修改的当前路径问题。不要为了“稳定显示”添加重试、删除缓存或假目录掩盖错误。

### 6.4 Codex CLI 升级已完成

终端 CLI 已从 0.152.0 升至官方 0.153.4，当前 `~/.local/bin/codex` 指向 standalone/current，新版已能解析现有配置。

```toml
[features]
context_management.experimental_mode = true
```

这段配置是有效语法。之前错误是旧 CLI 不支持嵌套表，不要改回 boolean；升级时配置文件内容未改。旧 release 目录保留用于回退。

## 7. 最新需求的代码定位与注意点

### 7.1 卡片化 Agent 页面

- `packages/views/agents/components/agent-detail-page.tsx`
  - `DetailHeader` 负责旧大横幅、头像名称、描述、状态与操作。
  - `handleDm`、`handleAssign`、archive/restore 与 `useAgentPermissions` 已有权限逻辑；搬按钮时保留这些行为。
  - `handleUpdate` 已有按字段的乐观更新及失败回滚，编辑卡片应复用。
- `packages/views/agents/components/agent-overview-summary.tsx`
  - 用户右侧参考卡片；当前是只读，含 owner、访问权限、Runtime、model、并发、Skills 与统计。
  - 需要改为完整身份/操作卡，移除不再符合需求的单 Agent Skills 区域。
- `packages/views/agents/components/agent-overview-pane.tsx`
  - 当前顶层为 overview / work / capabilities / settings。
  - capabilities 还包含 instructions、skills、mcp_config、composio_mcp、integrations。
  - 删除栏目时处理 URL `view`、导航意图和旧入口；不要留下空页面。
  - 全局集成配置与 Agent 专属配置是不同层级，别顺手删除用户没有要求删除的全局集成能力。
- `packages/views/agents/components/agent-detail-inspector.tsx`
  - 当前仍有头像、名称、描述的资料区域。
  - 头像/名称应移到卡片编辑；描述框应删除。
- 创建/复制入口：`create-agent-dialog.tsx`、`create/agent-configuration-panel.tsx`、`create/builder-conversation.tsx`、`create/builder-workspace.tsx`。

描述是展示字段，instructions 是可能影响执行的指令字段。用户要求 Agent 通用化，不能混为一谈；后续需追踪旧 Agent 专属 instructions 是否仍注入执行。不要误删平台系统指令、任务本身要求、项目 AGENTS.md 等不同来源的指令。

尚未决定旧数据库字段/绑定的处理方式，**没有做任何清空或迁移**。先调查当前消费者；保留历史数据不等于继续在新执行路径里偷偷生效。

### 7.2 删除不透明默认项

当前仍有三处明确入口：

- `model-selector-content.tsx`：模型“默认”。
- 同文件：思考强度“跟随 CLI 配置”。
- `model-speed-column.tsx`：“运行时默认”。

目录可提供 `RuntimeModel.default`、`thinking.default_level` 和 `supports_explicit_standard_service_tier`，但不是每个 Runtime 都支持显式配置。后续必须把真实选择/保存行为补齐：

- 不能只改标签，让空字符串继续代表未知值。
- 不能猜模型或随便写死 High / Standard。
- 对旧的空配置、没有强度选项的模型、托管模型 Runtime、首次引导和自动化的 model-only API，明确处理可选/不可选状态。
- 新验收是用户知道实际选择；此前“Default 可提交托管 Runtime”的测试不再是最终产品要求。

### 7.3 Skills / MCP 共享是真实执行语义变化

**不要只移动设置入口。** 当前源码已经确认：

- `packages/views/settings/components/mcp-tab.tsx` 的 workspace MCP 是一个库；注释明确写明“新增后不会自动给任何 Agent，需要到 Agent 页面分配”。
- `server/internal/handler/workspace_mcp_api.go` 有 `ListAgentMcpServers` 和每 Agent enabled 绑定。
- `server/internal/handler/daemon.go` 的普通任务 claim 在约 2186 行调用 `ResolveAgentMcpConfig`，需继续向上追踪 bindings 的来源。
- 同文件约 3303 行的 `ListWorkspaceMcpServers` 属于**自动化工具 allowlist**路径，不代表普通 Agent 已自动共享全部 MCP。
- `server/internal/service/task.go:LoadAgentSkills` 当前使用 `ListAgentSkills(agentID)`。
- 同文件有延迟解析 skill ID 的路径；改成工作区共享后，初次下发与后续 resolve 权限范围必须一致。
- 工作区 Skills 页面：`packages/views/skills/components/skills-page.tsx`；设置入口集中在 `packages/views/settings/components/settings-page.tsx`。

建议按“同一工作区统一管理、全部 Agent 可用”理解共享范围，并核对会员/管理员权限与 provider 实际能力。不要跨工作区共享；不要把本机 CLI 私有配置或环境变量里的密钥自动复制给其他机器。

已有 Agent 专属 MCP/Skills 绑定、直接 MCP 配置和工作区库项之间是否需要迁移、如何处理重名，需要根据当前实际数据与执行路径决定；目前没有实施，也没有承诺已经完成迁移。

### 7.4 环境变量说明

入口：`packages/views/agents/components/tabs/env-tab.tsx`。

当前只展示 key 数量；值需用户点击 Reveal & edit 后才读取。这条保密/按需读取规则不要破坏。

用户要说明它是运行进程的键值配置，可用于工具/API/代理等设置，与 Agent 描述/任务指令不同。文案应依据真实注入范围写清楚，不把它描述成修改整台机器的系统环境变量，也不要为了示例展示实际密钥。

### 7.5 设置返回整行点击与 Esc

入口：

- `apps/desktop/src/renderer/src/components/desktop-settings-page.tsx`
- 对应 `.test.tsx`
- 继续追踪 Desktop WindowOverlay / 全局快捷键处理后再放置 Esc handler。

已确认“返回应用”按钮是 `inline-flex` 且没有 `w-full`，只有文字/箭头宽度可点。它带 `WebkitAppRegion: no-drag`，修宽度时保留，否则拖拽区会吞点击。

本组件目前没有 Esc 监听。不要只写一个全局 capture handler 抢走所有 Esc：弹窗、菜单、快捷键录入或 IME 应先完成内层取消，再由设置关闭。验证按钮右侧空白和 Esc 都真正返回原应用页面。

## 8. 本地运行环境

| 项目 | 值 |
| --- | --- |
| Environment | `cordy-825` |
| Backend | `http://localhost:18905` |
| Electron renderer | `http://localhost:5999` |
| 当前唯一 Daemon profile | `desktop-localhost-18905` |
| Daemon health | `http://127.0.0.1:19599/health` |
| 停止的重复 profile | `dev-cordy-825`，不要再次同时启动 |
| PostgreSQL | 本地 PostgreSQL 17，127.0.0.1:55482 |
| 数据库 | `patchbay_cordy_825` |
| 数据目录 | `~/.cache/codex-tmp-10g/model-selector-postgres` |
| 工作区 slug / ID | `dev` / `179685d2-6982-4105-a5ae-663146ce75fb` |
| 测试 Agent | `Model Picker Verification` |
| 测试 Agent ID | `a50f42c8-43f4-42c5-b6b3-b8dfb46160e2` |
| 交接时 Agent 配置 | Codex / `gpt-5.6-sol` / `high` / `priority`（Fast），并发 6 |

测试 Agent 的配置在交接时通过 API GET 再次确认。未通过它发送推理任务或消息。

工具链：Node 22、pnpm 10.28.2、Go 1.26.6 已缓存。默认 shell 的 Node 可能是 26，因此显式设置 PATH。

```bash
cd /Users/alexjiang/.codex/worktrees/agent-model-three-column/Cordy
PATH="/opt/homebrew/opt/node@22/bin:/opt/homebrew/opt/postgresql@17/bin:$PATH"   GOTOOLCHAIN=go1.26.6 make up C=desktop
```

**不要再运行 `make up C=desktop,daemon` 来给这个环境另起独立 Daemon。** Electron 已经管理自己的 Daemon。

系统没有 Docker；本地 PostgreSQL 已正常运行。曾因 PATH 不含 PostgreSQL 工具导致启动脚本误以为数据库不可达并尝试 Docker。先检查 pg_isready/pg_ctl 与实际端口，不要新建、重置或删除已有数据库。

日志：

- `~/.patchbay/dev/envs/cordy-825/logs/desktop.log`
- `~/.patchbay/dev/envs/cordy-825/logs/api.log`
- `~/.patchbay/profiles/desktop-localhost-18905/daemon.log`
- `~/.patchbay/profiles/dev-cordy-825/daemon.log`（旧进程日志，已停止）

凭据仍在本地配置中；不要把 token、配置文件完整内容或含凭据的登录链接写进交接、日志输出或 PR。只在程序内读取并放入请求头。

当前沙箱允许读整个文件系统，但本任务 worktree 位于常规可写根以外，写文件、运行写缓存/改 Git 的命令需要 `require_escalated` 自动审批。不要为了绕过这个限制改到用户的 main 工作目录。

## 9. Electron 验证方法与已踩过的坑

按用户最新 AGENTS.md：**产品验收用 Electron，Web 仅用于 Web 专属平台接线。** 不要拿浏览器 demo 代替。

实际应用路径：

```text
/Users/alexjiang/.codex/worktrees/agent-model-three-column/Cordy/node_modules/.pnpm/electron@39.8.7/node_modules/electron/dist/Electron.app
```

显示名称 `Orvilo Canary cordy-825`，bundle ID `ai.patchbay.desktop.canary.dcafbf3ca5ec0a35`。

- 用当前 Computer Use API 读取状态后再操作；不要复用旧 accessibility index。
- 曾多次遇到用户正在操作同一应用、窗口缩小/切换页，导致 “The user changed” 或旧索引无效。
- 高效且实际成功的方式：同一次调用内读取最新完整 AX、按已观察到的标签定位、点击、再读取状态，然后再进行下一步。
- `getAXState({ emit: false, disableDiffing: true })` 可得到完整文本供定位；每次点击后都重新读状态。
- 不要调用不存在的 `modelApp.getState()`。
- 若用户正在填写表单，不要关闭或覆盖。此前已经为一轮短暂验证协调过，但这不等于以后可以破坏新的未保存输入。
- 偶发整窗空白曾通过**只重启 desktop 组件**解决；不要据此重置数据库或其他 worktree。

已实际验证的界面结果：方形完整灰底、四列布局、High 保存、Fast 勾选与成功提示；随后 API readback 确认 `model=gpt-5.6-sol / thinking_level=high / service_tier=priority`。这不等于后来提出的卡片化页面等需求已经验证。

## 10. 验证与下一步

第 3 节清单已在本 worktree 验收（Electron 截图 + claim 测试 + 定向 vitest）。不要合并 PR。清单代码尚未 commit / push；GitHub 上已推送的 CI 只覆盖四列选择器两个 commit，不能代替未推送改动。

用户明确要求后再提交。不要再为 `cordy-825` 另起独立 Daemon。不要动 PR769。
