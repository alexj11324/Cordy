# 全量迁移总体方案 W4–W9（Cordy-go-patchbay Go 主线）

> 基准：已合入 tip `6d58c4b9a`（W3 roles），Go ledger 止于 `455`。新迁移从 **456+** 起，编号连续、无间隔；不复用 Cordy Rust 文件名（Rust 侧已用至 480+）。每波均为「新 Go 迁移 + sqlc 再生 + Go（handler/service/daemon/CLI）+ packages/core + packages/views + apps/web & desktop & mobile + 四语 i18n + 必填测试 + 文档」；新 Pg 索引必须 `CREATE [UNIQUE] INDEX CONCURRENTLY` 且独立单语句文件；无 FK/cascade；硬切旧名无双写。参考 remaining-handoff 与 `01a05e9c-…/goal/plan.md`。

## 0. 已有 W4 草稿盘点（本仓库未提交）

5451

`456_agent_task_execution_lane_key`（STORED 计算列 `execution_lane_key`）\n`457_agent_task_execution_lane_active_unique`（CONCURRENTLY 部分唯一索引，`status IN (dispatched, running, waiting_local_directory)`）\n`458_task_capability_lease`（W4 能力面 2689 行：`authorization_grant` / `authorization_audit_event` 创表 + `task_token` 扩列 + 回填/去重/排序 + 两组触发器/函数 + 注释；Rust 对应 411）\n459–464 四类索引：`authorization_grant` ×2、`authorization_audit_event` ×2、`task_token` parent/claim-fence ×2（对应 Rust 412–417）\nhandler/daemon/sqlc 已对齐该草稿（`daemon.go` 双 claim 路径、`task_token.sql` 的 claim/parent/lease CTE、`agent.sql` 的 lane 化 `ClaimAgentTask`、生成物 `models.go`/`task_token.sql.go`）。

判定：W4 草稿基本可用，需一次合规复核（单语句索引、幂等 `IF NOT EXISTS`/`IF EXISTS`、`CONCURRENTLY` 不在显式事务中、down 可逆、注释与触发器语义）后再作为 456–464 正式提交基线。

## 1. 编号总体图（Go 侧拟定）

```
已提交:  … 446(Teams) 447(Automation) 448-449(Patrick) 450-455(roles: owner/executor/reviewer)
草稿  :  456 lane-key │ 457 lane-unique │ 458 capability-lease+auth-foundation │ 459-464 auth/token 索引
W5    :  465 dependency_graph_domain │ 466-473 图索引（plan/node/edge/幂等/parent/temp/direction/from/to） │ 474 execution_gate │ 475 attention │ +（按需）480-483 增量图补丁
W6    :  476 work_product + relation + provenance（含 430-445 对应）│ 477-483 配套索引与清理（external-id/provider/relation-key/manifest 任务等） │ +（按需）484-489 发现队列/分支索引
W7    :  490 agent-thread 统一（不再使用 LobeHub sidebar；PR #660 为误合入已在后续纠正，回归原生侧栏；必要时补表/列，尽量复用现有 chat 栈）
W8    :  491 workspace channels + Weixin（复用 channel/wecom，不另起栈；对应 Rust 386-393/微信增量）
W9    :  492 guest session + 493 clerk/oauth/google + 494 auth-broker handoff（patchbay://）│ 合并 PR #727 前将 POST /auth/google 移出 Web 主路径（保留 mobile send-code）
```

> W5/W6 设计可在 W4 落地后并行，但**提交顺序仍先 W5 后 W6**（remaining-handoff 约束）。若某波 Cordy 后续有小补丁（如图 `attention` 485、executor fields 495 等），在对应波次尾部追加编号，不打乱已定主号。

## 2. W4 执行面（先闭环）

- 迁移：以现有 456–464 草稿为基线，复核后原样提交（内容见 §0；Rust 对应 408–417）。
- sqlc：`server/pkg/db/queries/agent.sql`（lane claim）、`task_token.sql`（lease/audit 能力）、生成物；`make sqlc`。
- Go：`ClaimAgentTask` 绑定 lane 串行、`task_token` 能力写入与校验、`daemon` 认领链路、不改 26 个 runtime 适配器实现。
- 测试：真实 `httptest` 打 claim/token 链路（包含 lane 冲突、失效/吊销、审计落盘）。
- 每波 DoD：`pnpm typecheck`、`set -a && . ./.env && set +a && cd server && go test`（分包跑，避开 shared-DB 序号冲突），GH 必过（W1 已红的 sqlc-check/frontend-* /backend-tests）。

## 3. W5 依赖图（预置）

- Rust 对照：**418 domain**（`dependency_graph_plan/node/edge` 三表，无 FK），**419–427 单索引**（plan_id / node_id / edge_id / idempotency_key / active_parent / node_temp / edge_direction/from/to），**428 execution_gate**（`dependency_graph_issue_gate_open` + `trg_dependency_graph_task_admission` 绑 `agent_task_queue` 入队/状态变更），**429 attention**（`attention_required/reason`），**448–449** `dependency_graph_issue_created_outbox`，**459** `agent_task_execution_target`，**460** `dependency_graph_executor_fields`，**463** `dependency_graph_roles_target`。
- Go 预置：`465_dependency_graph_domain` 建主表 + `466–473` 单索引 + `474_execution_gate` + `475_attention`，后续增量出 480+ 槽位（outbox/executor/roles 等按需追加）。
- 约束要点：无 FK/cascade；重跑幂等（`IF NOT EXISTS`）；执行门闸与 W4 `execution_lane_key` 共存——lane 负责并发互斥，图负责前置依赖闭合；变更均由触发器在 DB 侧强制，handler 不得旁路。
- 前端：`packages/core/dependency-graphs` + `packages/views/task-graph` + 三端路由 + 四语（含 task-graph 菜单项）。

## 4. W6 Work products & Provenance

- Rust 对照：**430 work_product**（`id/workspace_id/kind/provider/external_identity/url/provider_record`），**431–434** 索引（pk/external_identity/provider_record），**435 relation**（`work_product_relation` 含 `issue/task/run/relation_key/relation_source/attached_by/close_intent/detached_*` 多重 CHECK），**436–441** relation 索引（pk/relation_key/issue/product/task），**442 provenance**（`agent_task_execution_provenance` 含 `repo_identity/execution_workspace/head_branch/sha/state/discovery_*`），**443–445** provenance 索引，**446–447** 旧 `pull_request` 清理，**450** discovery queue 索引。
- Go 预置：`476_work_product_and_provenance` 主表簇（work_product + relation + provenance）+ `477–483` 索引/关联键 + `484–489` 发现/分支增量，按需拆分；API 与 UI 同时可见；同样无 FK。
- 约束要点：`work_product_relation` 的多重 CHECK（至少一锚点、source/attached_by 绑定、run→task、detached 完整性）需原样落地；`provenance` 的 `repo_identity/head_state` 等 CHECK 保留；W4 的 `task_token` 能力链与 provenance 发现流程解耦。

## 5. W7 Agent Thread（不再使用 LobeHub sidebar）

- 原方案中的 LobeHub 侧栏为 PR #660 误合入，已在后续纠正，本计划不再引入 `@lobehub/ui` 侧栏；W7 仅做 agent-thread 统一与原生侧栏回归（对照 `docs/agent-thread-automation-migration.md` 中非 LobeHub 部分）。Go 侧多为前端/路由/i18n 增量，若需持久化则补最小列/索引，不建新栈。

## 6. W8 Channels + Weixin

- 对照：Rust **386** `workspace_channel`（`id/workspace_id/slug/name/status`）、**387–389** channel 索引（hub_unique/slug/pk），**390–393** `workspace_channel_message` 栈及 Weixin/WeCom 增量；Go 侧复用现有 `channel`/`wecom` 抽象，补产品面与微信绑定面。

## 7. W9 Auth（收口）

- 新增：`492_guest_session`（Rust **397** `user.is_guest` + **398–400** `guest_session` 表/索引，token 仅存 SHA-256 hash）、`493_clerk_oauth_google`（Clerk + `/oauth/google` + guest 主路径）、`494_desktop_auth_broker`（`apps/auth-broker` + `patchbay://auth/callback`）。
- 移除：`POST /auth/google` 不再为 Web 主路径（保留 server 侧兼容或删除，以路由审计为准；mobile 保留 send-code）。
- 合并：待 GH 必过全绿后按仓库 required merge 方式合入 main；若遇 branch protection/审批/密钥/回调阻塞，以 `blocking: "unverifiable"` 停止，不伪造验证。

## 8. 校验清单（每波）

- 迁移 `migrations-audit.txt`：编号连续、无 Rust 重名、CONCURRENTLY 单语句、`IF EXISTS/NOT EXISTS` 幂等。
- `legacy-grep.txt`：除历史/rename down 外无 Autopilot/Squads/Mika 规范残留；W9 后 Web 无 `POST /auth/google` 主路径。
- 本地：`pnpm typecheck`、`go test`（sourced .env）、抽样 handler 覆盖该波契约；GH Actions 全绿为合并权威。
- 提交：conventional commit（English），push `feat/go-mainline-patchbay` → PR #727，每波一 commit，不跨波合批。

## 9. 下一步

1. 复核 456–464 执行合规并原样提交为 W4 基线，sqlc 再生、窄测绿。
2. 按序建 465+ 后续槽位，每波独立实现与提交。

