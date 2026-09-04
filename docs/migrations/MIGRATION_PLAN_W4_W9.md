# W4–W9 Rust → Go migration plan（历史完成记录）

> 状态：已归档。W4–W9 阶段已经结束；本文只保留迁移期间的范围、编号和约束
> 作为历史完成证据，不再是当前执行说明，也不把已退出的 Rust tree 当作
> shipping authority。

本文记录的是当时 Go 主线的迁移拆分。Rust 代码仅作为只读对照快照，当前判断
以 Go shipping 源码、现行 API/database contracts 和
[Rust → Go parity audit](RUST_TO_GO_PARITY_AUDIT.md) 为准。当前 issue 角色的
canonical 语义是：member 作为 owner，agent/team 作为 executor，
member/agent/team 作为 reviewer；旧的泛化角色称呼不再是当前 contract 名称。

## 0. W4 历史草稿盘点

5451

- `456_agent_task_execution_lane_key`（STORED 计算列 `execution_lane_key`）。
- `457_agent_task_execution_lane_active_unique`（CONCURRENTLY 部分唯一索引，`status IN (dispatched, running, waiting_local_directory)`）。
- `458_task_capability_lease`（W4 能力面：`authorization_grant` / `authorization_audit_event` 创表、`task_token` 扩列、回填/去重/排序、触发器/函数和注释；Rust 对应 411）。
- 459–464 四类索引：`authorization_grant` ×2、`authorization_audit_event` ×2、`task_token` parent/claim-fence ×2（对应 Rust 412–417）。

历史记录当时称 handler/daemon/sqlc 已对齐该草稿；这句话只描述当时的
W4 记录，不是今天的生成或验收结论。

历史判定：W4 草稿当时被记录为基本可用，仍需按当时的单语句索引、幂等性、
`CONCURRENTLY`、可逆 down 和触发器语义约束复核；这不是今天的提交指令。

## 1. 编号总体图（历史记录）

```
已提交:  … 446(Teams) 447(Automation) 448-449(Patrick) 450-455(roles: owner/executor/reviewer)
草稿  :  456 lane-key │ 457 lane-unique │ 458 capability-lease+auth-foundation │ 459-464 auth/token 索引
W5    :  465 dependency_graph_domain │ 466-473 图索引（plan/node/edge/幂等/parent/temp/direction/from/to） │ 474 execution_gate │ 475 attention │ +（按需）480-483 增量图补丁
W6    :  476 work_product + relation + provenance（含 430-445 对应）│ 477-483 配套索引与清理（external-id/provider/relation-key/manifest 任务等） │ +（按需）484-489 发现队列/分支索引
W7    :  490 agent-thread 统一（不再使用 LobeHub sidebar；PR #660 为误合入已在后续纠正，回归原生侧栏；必要时补表/列，尽量复用现有 chat 栈）
W8    :  491 workspace channels + Weixin（复用 channel/wecom，不另起栈；对应 Rust 386-393/微信增量）
W9    :  492 guest session + 493 clerk/oauth/google + 494 auth-broker handoff（patchbay://）│ 合并 PR #727 前将 POST /auth/google 移出 Web 主路径（保留 mobile send-code）
```

历史决策：W5/W6 当时允许并行设计，但提交顺序记录为先 W5 后 W6；后续小补丁
不应从本文推导新的编号或提交顺序。

## 2. W4 执行面（历史记录）

- 历史范围：456–464 覆盖 execution lane、capability lease、authorization/token
  索引和对应的 Go handler/daemon/sqlc 链路。
- 历史验收要求：包括 claim/token 的 `httptest`、lane 冲突、失效/吊销、审计落盘，
  以及前端检查和 GitHub Actions；这些命令不构成当前执行指令。

## 3. W5 依赖图（历史设计与完成范围）

- Rust 对照：**418 domain**（`dependency_graph_plan/node/edge` 三表，无 FK），**419–427 单索引**（plan_id / node_id / edge_id / idempotency_key / active_parent / node_temp / edge_direction/from/to），**428 execution_gate**（`dependency_graph_issue_gate_open` + `trg_dependency_graph_task_admission` 绑 `agent_task_queue` 入队/状态变更），**429 attention**（`attention_required/reason`），**448–449** `dependency_graph_issue_created_outbox`，**459** `agent_task_execution_target`，**460** `dependency_graph_executor_fields`，**463** `dependency_graph_roles_target`。
- Go 历史实现范围：`465_dependency_graph_domain` 主表、`466–473` 单索引、
  `474_execution_gate`、`475_attention`，以及后续 outbox/executor/roles 增量。
- 约束要点：无 FK/cascade；重跑幂等（`IF NOT EXISTS`）；执行门闸与 W4 `execution_lane_key` 共存——lane 负责并发互斥，图负责前置依赖闭合；变更均由触发器在 DB 侧强制，handler 不得旁路。
- 前端：`packages/core/dependency-graphs` + `packages/views/task-graph` + 三端路由 + 四语（含 task-graph 菜单项）。

## 4. W6 Work products & Provenance（历史记录）

- Rust 对照：**430 work_product**（`id/workspace_id/kind/provider/external_identity/url/provider_record`），**431–434** 索引（pk/external_identity/provider_record），**435 relation**（`work_product_relation` 含 `issue/task/run/relation_key/relation_source/attached_by/close_intent/detached_*` 多重 CHECK），**436–441** relation 索引（pk/relation_key/issue/product/task），**442 provenance**（`agent_task_execution_provenance` 含 `repo_identity/execution_workspace/head_branch/sha/state/discovery_*`），**443–445** provenance 索引，**446–447** 旧 `pull_request` 清理，**450** discovery queue 索引。
- Go 预置：`476_work_product_and_provenance` 主表簇（work_product + relation + provenance）+ `477–483` 索引/关联键 + `484–489` 发现/分支增量，按需拆分；API 与 UI 同时可见；同样无 FK。
- 约束要点：`work_product_relation` 的多重 CHECK（至少一锚点、source/attached_by 绑定、run→task、detached 完整性）需原样落地；`provenance` 的 `repo_identity/head_state` 等 CHECK 保留；W4 的 `task_token` 能力链与 provenance 发现流程解耦。

## 5. W7 Agent Thread（历史记录；不使用 LobeHub sidebar）

- 原方案中的 LobeHub 侧栏为 PR #660 误合入，已在后续纠正，本计划不再引入 `@lobehub/ui` 侧栏；W7 仅做 agent-thread 统一与原生侧栏回归（对照 `docs/agent-thread-automation-migration.md` 中非 LobeHub 部分）。Go 侧多为前端/路由/i18n 增量，若需持久化则补最小列/索引，不建新栈。

## 6. W8 Channels + Weixin（历史记录）

- 对照：Rust **386** `workspace_channel`（`id/workspace_id/slug/name/status`）、**387–389** channel 索引（hub_unique/slug/pk），**390–393** `workspace_channel_message` 栈及 Weixin/WeCom 增量；Go 侧复用现有 `channel`/`wecom` 抽象，补产品面与微信绑定面。

## 7. W9 Auth（历史记录）

- 新增：`492_guest_session`（Rust **397** `user.is_guest` + **398–400** `guest_session` 表/索引，token 仅存 SHA-256 hash）、`493_clerk_oauth_google`（Clerk + `/oauth/google` + guest 主路径）、`494_desktop_auth_broker`（`apps/auth-broker` + `patchbay://auth/callback`）。
- 移除：`POST /auth/google` 不再为 Web 主路径（保留 server 侧兼容或删除，以路由审计为准；mobile 保留 send-code）。
- 历史备注：合并、branch protection、审批、密钥、回调和真实运行时验收均不由
  本文证明；应以当前 parity audit 和实际 CI/deployment 证据判断。

## 8. 历史验收约束

- 历史约束：迁移编号、索引 `CONCURRENTLY`、`IF EXISTS/NOT EXISTS`、legacy grep、
  前端检查和 GitHub Actions 曾被列为各波验收项目。
- 历史约束：每波应使用 conventional commit；本文不保留分支、PR 或当前提交
  操作指令。

## 9. 当前来源

当前差距、证据和后续 acceptance 统一写在 [Rust → Go parity audit](RUST_TO_GO_PARITY_AUDIT.md)
中；实现以 Go shipping tree 和现行 contracts 为准。本文不提供下一步命令、
迁移编号或 Rust 文件路径来指导新工作。
