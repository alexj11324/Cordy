# Go→Rust 全量迁移：独立基线审计与执行 TODO

> 审计快照：2026-08-27 UTC
> 审计基线：0f92fb042ffc742b8dcf8af91cea3d97716c05a4
> 首次审计交付：Ready PR #521
> 范围：server/、server-rs/、默认运行/构建/发布/部署链路

这是一份独立的全局基线，不是按“完成一块再查一块”生成的局部记录。后续迁移只能从本清单选择切片；完成切片后更新对应证据和状态，再选择下一块。它取代 tasks/go-to-rust-migration.md 中互相矛盾的当前状态判断；旧文件保留为历史执行记录。

本文件同时是迁移的唯一执行台账：所有未迁移、未接线、未验证或不能删除 Go
的工作都必须在这里有唯一 ID。下方“执行台账”负责当前状态和下一动作，P0/P1
条目负责范围与退出证据；两者冲突时以执行台账为准。新的缺口先补进台账，再
开始代码变更；不允许在台账之外临时开工。

## 1. 执行边界

主 agent 只做：

- Go→Rust 业务能力或完整契约迁移；
- Rust 生产入口接线；
- worktree、分支、提交、推送和 Ready PR；
- 接收异步结果后回写台账和 PR。

独立 verification agent 异步负责：

- 编译、测试、格式/静态检查、契约检查和生产入口验证；
- 只报告精确命令与结果，不 review、不修改或修复代码。

独立 reviewer agent 异步负责：

- review 迁移完整性、生产 wiring、证据和 Ponytail；
- 只输出 findings，不修改或提交代码。

独立 fixer agent 异步负责：

- 根据 review findings 修复缺陷、安全问题和回归；
- 修复机械验证发现的编译或测试失败；
- 必要时直接提交修复。

verification、reviewer 与 fixer 必须由三个不同的 subagent 承担。主 agent 派发后
继续迁移，不等待、不轮询，也不把任何异步结果作为下一块迁移、提交、推送或
Ready PR 的前置条件；长时间编译和测试不得占用主 agent 的迁移主线。

## 2. Ponytail 约束

本迁移遵循 Ponytail：

- 优先复用现有 crate、依赖和生产入口；
- 只有迁移本身、真实编译/安全问题或明确生产边界需要时才拆文件；
- 禁止按“一文件一职责”机械拆分；
- 不做与当前 Go 能力迁移无关的清理；
- 对非平凡解析器、协议、并发和安全边界保留可运行的契约/回归检查；
- 安全的 Noop/Stub 只允许用于测试或明确的 fail-closed 分支，不能用它们冒充生产迁移完成。

## 3. 审计口径

本表把四种状态分开：

| 状态 | 含义 |
| --- | --- |
| Rust 已落地 | Rust 中有对应业务能力或完整契约；文件数量不作为一一对应证明 |
| Rust 已接线 | Rust server/CLI/daemon 的内部生产 assembly 能调用该能力 |
| 默认生产已切换 | Makefile、脚本、Docker、Helm、release/install 和实际启动命令默认执行 Rust |
| Go 可下线 | 默认生产路径、发布产物、契约验证和回滚演练都不再依赖 Go，且可以删除 Go 源文件 |

审计证据优先级：

1. 当前 source、调用关系和实际默认入口；
2. 可复现的编译、测试、契约检查和生产 smoke；
3. CI/release/deploy 配置；
4. Git 历史和旧计划文档。

“Rust 文件存在”“route parity 通过”都不能单独证明生产已经切换；在本快照中还没有任何 Go 目录可以标记为可下线。

## 4. 全局基线盘点

### 4.1 源码规模

以下数字由基线 commit 的 Git tree 统计，Go 行数只统计非测试源文件；Rust 行数包含 source 文件中的 inline tests。

| 范围 | 文件 | 行数/备注 |
| --- | ---: | --- |
| Go server 全部 | 1,445 | 其中 807 个 _test.go |
| Go 非测试源 | 638 | 276,704 行 |
| Go cmd | 53 | 26,311 行；server 14、cordy 35、migrate 1、backfill 3 |
| Go internal | 439 | 176,572 行 |
| Go pkg | 146 | 73,821 行 |
| Rust crates/src | 527 | 330,127 行，包含 inline tests |
| Rust 外部 integration tests | 5 | 不含 inline tests |

Rust 不是 Go 文件的机械镜像。当前最大的 Rust 落点是：

| Rust crate | source 文件 | 行数 | 主要 Go 来源 |
| --- | ---: | ---: | --- |
| cordy-handler | 84 | 70,788 | cmd/server/router、internal/handler |
| cordy-daemon | 74 | 52,433 | internal/daemon、internal/daemonws |
| cordy-db | 59 | 35,642 | pkg/db、db migrations/query contract |
| cordy-cli | 6 | 30,655 | cmd/cordy、internal/cli |
| cordy-agent | 31 | 27,123 | pkg/agent |
| cordy-service | 39 | 25,880 | internal/service 及多个 leaf package |
| cordy-wecom | 24 | 12,118 | internal/integrations 的 WeCom 域 |
| cordy-lark | 29 | 13,968 | internal/integrations 的 Lark 域 |

### 4.2 主要能力矩阵

| Go 能力/来源 | Rust 落点 | Rust 内部生产入口 | 默认生产路径 | 当前判断与剩余工作 |
| --- | --- | --- | --- | --- |
| HTTP router、handler（internal/handler 111 文件） | cordy-handler、cordy-server | cordy-server::build_production_router | 否，Makefile/Docker/scripts 仍启动 Go | 路径契约已完整覆盖；行为 smoke、cutover、Go 删除未完成 |
| DB models/query、migrations（pkg/db、cmd/migrate） | cordy-db、cordy-migrate | cordy-server 使用 cordy-db；cordy-migrate 有 up/down/status | 否，Makefile、Docker entrypoint、Helm 仍调用 Go migrate | Rust runner 已落地；发布/启动/锁/回滚验证待 AUDIT-001、006 |
| auth、middleware、realtime、metrics | cordy-auth、cordy-middleware、cordy-realtime、cordy-metrics | cordy-server main 和 handler state | 否 | Rust 能力已落地并在 Rust assembly 使用；跨入口行为和 cutover 待验证 |
| service、feature flags、entitlement、issue/task/autopilot | cordy-service 及 handler | cordy-server 配置 entitlement、handler state 安装 provider | 否 | 主要能力已接线；需要与 Go contract、错误/缓存/策略行为做 smoke 对账 |
| integrations（130 个非测试 Go 文件） | channel/channel-engine、Lark、WeCom、DingTalk、Slack、Telegram、Composio、VCS、GHSnapshot | cordy-server::channel_runtime、handler::connectors | 否 | Rust 真实 wiring 已存在；真实配置、缺失配置 fail-closed、各 provider smoke 未形成矩阵 |
| agent providers（pkg/agent 53 文件） | cordy-agent，registry + consolidated backends | cordy-daemon provider adapter、cordy-cli daemon | 否，release 仍 Go CLI | provider registry 已覆盖主要 Go provider id；完整命令/环境/退出码矩阵待验证 |
| daemon、daemonws | cordy-daemon | cordy-daemon::run_production_daemon、cordy-cli daemon | Rust CLI 内部已接线，默认发布仍 Go | 生产 stack 存在，但有 43 条 S9-integration 标记、28 个文件受 dead_code allow 影响；需按能力验证，不做机械清标 |
| local/S3/CloudFront storage | handler::attachment_storage、cloudfront、attachment、avatar | cordy-server main 注入 attachment storage；squad CRUD 复用 cordy-handler::avatar | 否 | 主存储能力已落地/接线；Squad avatar 的读写 URL 契约已接线，剩余发布/全量 Go 退休仍未闭环 |
| CLI bins（cordy、migrate、3 backfill） | cordy-cli、cordy-migrate 及 3 个 Rust backfill bin | Rust bin 可独立运行 | 否，Makefile/Docker/release 仍产出 Go | Rust bin 已存在；构建产物、命令行为、安装/发布和 Docker packaging 未闭环 |
| pprof、logger | Go internal/profiling、internal/logger | cordy-server::profiling 已接入 loopback CPU pprof；cordy-util logging、cordy-server、cordy-migrate、cordy-daemon 和 request middleware 已接线 | Rust 默认后端/daemon 已使用 | CPU/cmdline/symbol 已有 Rust listener；heap/trace 和 logger 的人类可读时间布局、剩余发布路径仍未闭合 |
| Go tests（807 文件） | Rust inline tests + 5 个外部 integration test 文件 | CI 同时运行两套 | CI 验证不等于生产切换 | 不能机械改写 807 个文件；需按业务契约建立覆盖矩阵，见 AUDIT-007 |

### 4.3 Go leaf package 对账

这一表用于防止“文件名变少”被误判为“能力丢失”，也用于标记真正需要补证据的 leaf。

| Go 来源 | Rust 落点 | 结论 |
| --- | --- | --- |
| internal/analytics | cordy-analytics | 已落地；Noop 仅作无配置/测试路径，需生产配置 smoke |
| internal/attribution、attributionbackfill | cordy-service::attribution、cordy-migrate::backfill::attribution | 已落地；需随迁移 runner 验证 |
| internal/auth | cordy-auth | 已落地/接线 |
| internal/agentconfig | handler agent_api inline 校验、daemon config | 能力已出现；默认值/边界需统一契约证据，不能凭重复常量宣布完成 |
| internal/channelmedia | cordy-util::channel_media | 已落地 |
| internal/cloudruntime | handler::cloud_runtime | 已落地/接线 |
| internal/dispatch | cordy-service::dispatch_reason | 已落地 |
| internal/entitlement | cordy-service::entitlement、autopilot provider | 已落地并由 cordy-server 注入 |
| internal/events | cordy-events | 已落地 |
| internal/featureflags | cordy-service::feature_flags | 已落地；此前契约测试 9/9，需要纳入总矩阵 |
| internal/issueactivitybackfill | cordy-migrate::backfill::issue_activity | 已落地为 Rust bin；发布 packaging 未闭环 |
| internal/issueguard、issueposition、issuestatus | cordy-service/handler 对应模块 | 已落地；需行为 contract smoke |
| internal/logger | cordy-util::logging、cordy-server/cordy-migrate/cordy-daemon tracing 初始化、cordy-middleware request span | 吸收式迁移已接线；LOG_LEVEL、TTY、daemon component 和请求属性已对齐，默认 tracing 时间布局仍需单独决定是否作为兼容契约 |
| internal/migrations | cordy-migrate runner/hooks | 已落地；默认入口仍 Go |
| internal/profiling | cordy-server::profiling | CPU/cmdline/symbol 已迁移并由 Rust server 启动；heap/trace 仍未闭合，见 AUDIT-003 |
| internal/runtimeapps、selfexec、util | cordy-service、cordy-daemon::update_executor、cordy-util/daemon | 已吸收式落地；不拆重复小 PR，按调用路径验证 |
| internal/taskusagebackfill | cordy-migrate::backfill::task_usage | 已落地为 Rust bin/hook；发布 packaging 未闭环 |
| internal/testutil | Rust 各 crate 测试辅助 | 非生产能力，纳入测试矩阵，不单独制造业务 PR |
| pkg/dbid | cordy-db::dbid | 已落地 |
| pkg/llm | cordy-llm | 已落地 |
| pkg/plugincontract | cordy-plugincontract | 已落地 |
| pkg/protocol | cordy-protocol | 已落地 |
| pkg/redact、skillbundle | cordy-service 对应模块 | 已落地 |
| pkg/remotemcp | cordy-remotemcp、daemon broker | 已落地/接线证据存在 |
| pkg/taskfailure | cordy-task-failure | 已落地 |

## 5. 已核实的强证据

### 5.1 HTTP route contract

执行：

    python3 server-rs/scripts/route_parity.py --require-complete

结果：

    Go contract: 424 | Rust: 424 | covered: 424 | missing: 0 | extra: 0

这证明 method/path 组合已经覆盖，不证明每个 handler 的响应、权限、事务、副作用和默认进程入口都等价。

### 5.2 Rust server、daemon、CLI assembly

- cordy-server main 加载 Config、构建 HandlerState、注入 storage/metrics/entitlement/channel runtime，并启动 Axum production router。
- cordy-daemon 有 DaemonProductionAssembly、ProductionProviderAdapter、ProductionServices、ProductionStack。
- cordy-cli 有 Agent、Issue、Auth、Workspace、Runtime、Autopilot、Skill、Squad、Daemon、Update 等命令族，并调用 Rust daemon production entry。
- cordy-agent registry 已包含 ACP、Antigravity、Claude、CodeBuddy、Codex、Copilot、Cursor、Deveco、Dim、DSH、Grok、Hermes、Kimi、Kiro、Mcode、OMP、OpenClaw、OpenCode、Pi、Qoder、QwenPaw、Reasonix、TraeCLI 等主要 provider。

这些是“Rust 内部已实现/已接线”证据，不是“默认生产已切换”证据。

### 5.3 Integration 和 fallback 边界

- Lark 生产 runtime 使用真实 HttpApiClient；StubApiClient 会显式报未配置错误，不静默成功。
- Lark/其他 channel 的 Stub/Noop 主要服务测试、缺配置或 fail-closed 保护，必须在 AUDIT-004 中证明不会被有效生产配置错误地选中。
- WeCom、DingTalk、Slack、Telegram 等 Rust crate 和 handler connector 已存在；旧计划中“未移植”的部分与当前 source 不完全一致，不能继续按旧 checkbox 派工。

## 6. 未完成执行 TODO

优先级含义：P0 是删除 Go 前必须完成的生产闭环；P1 是 cutover 前必须完成的兼容性/运维证据。每个 ID 之后应由一个可合并业务切片或一个明确的验证/发布切片收口，不按文件数拆 PR。

### 6.1 唯一执行台账

状态只描述主线交付，不把异步 verification/review/fix 当作阻塞状态。ID 是稳定的
能力轨道标识，不是必须按数字完成的流水阶段；选择顺序由“依赖/可执行门”决定：

| 完成 | ID | 状态 | 已交付/当前切片 | 下一动作与退出缺口 | 依赖/可执行门 | 证据/PR | owner |
| --- | --- | --- | --- | --- | --- | --- | --- |
| [~] | AUDIT-001 | 进行中 | 默认 server、CLI、migration、Docker、CI、Helm、CLI release 资产链、Desktop 内嵌 CLI、tag release 验证门、self-host exact-image rollback、opt-in systemd 生命周期与 required backend CI Go gate 已切到 Rust | 收口异步 finding；随后执行真实启动/升级/回滚演练 | release/installer/systemd/CI gate 已交付；最终生产验收依赖 AUDIT-002..009 退出 | PR #523/#527/#551..#554；详见 §11、§15、§16、§38..§41 | 主 agent；独立 V/R/F subagent |
| [~] | AUDIT-002 | 进行中 | route parity、CLI/daemon matrix、issue-status #565、issue create #566、user WebSocket #567、scheduler #568、heartbeat #569、stale sweeper #570、offline-task/reconnect-retry #571、stale/queued cleanup #572 与 delegated recovery #574 Ready | 收口 #565..#574 的异步 V/R/F 结果，同时继续下一项完整 background-worker 契约；异步结果不阻塞主线 | 复用唯一 Rust production assemblies；#572 堆叠在 #571、#574 堆叠在 #573；主线不等待 verifier/reviewer/fixer | PR #565..#574；§5、§6.2、§18、§52..§61 | 主 agent；独立 V/R/F subagent |
| [~] | AUDIT-003A | Ready PR | CPU/cmdline/symbol pprof 已接入；PR #556 的 Linux process telemetry 保留为趋势指标；PR #560 迁移真实 allocation-stack heap profile 与 Rust async runtime diagnostics | 异步收口 Cargo.lock、Linux/non-Linux/Docker 构建、真实 pprof/console client、public isolation、shutdown 与开销证据，finding 交 fixer | Rust server/profiling 入口可执行；依赖当前稳定 Rust、Linux release 构建和可写临时目录 | PR #524/#556/#560；详见 §12、§43、§47 | 主 agent；独立 V/R/F subagent |
| [~] | AUDIT-003B | Ready PR | logger 配置、TTY、component、request attrs 与本地毫秒时间布局已接入全部 Rust production subscriber | 异步验证真实输出、daemon rotating sink、timezone/DST与既有行为无回归，finding 交 fixer | Rust server/daemon/migrate/backfill 入口可执行 | PR #525/#557；详见 §13、§44 | 主 agent；独立 V/R/F subagent |
| [~] | AUDIT-003C | Ready PR | squad avatar 读写已接入既有 avatar capability | 等待异步 V/R/F，并纳入生产对象存储 smoke | 依赖 AUDIT-004 的生产存储证据完成退出 | PR #526；详见 §14 | 主 agent；独立 V/R/F subagent |
| [~] | AUDIT-003D | Ready PR | agent 的每实体限额已集中为默认 6、范围 1..50；daemon 的进程级 slot pool 独立保持默认 20、要求 >0 | 等待异步 V/R/F；生产 daemon 生命周期 smoke 继续归 AUDIT-005 | 配置契约可执行；最终退出依赖 AUDIT-005 daemon 生命周期 | PR #531；§6.2、§19 | 主 agent；独立 V/R/F subagent |
| [~] | AUDIT-004 | 主线切片已交付 | Lark、WeCom、DingTalk、Slack、Telegram、Composio、VCS、GHSnapshot 与 channel media production lifecycle 已交付 | verification 收口 supervisor/lease 矩阵、外部凭证 smoke/不可测原因与回滚策略；review/fix 异步回写 | 主 agent 当前无新的不重叠迁移缺口；最终退出依赖异步 V/R/F 直接证据 | PR #532..#536/#538..#541；§5.3、§6.2、§20..§28 | 主 agent；独立 V/R/F subagent |
| [~] | AUDIT-005 | 进行中 | `/health`、provider refresh、GC metadata、runtime/Remote/plugin-hook MCP、local-skills、wakeup/control、auto-update、poisoned-session、Codex rollout durability、confirmed provider demotion/recovery、private task temp 与 wakeup environment proxy production chain 已交付；heartbeat HTTP pool recovery 已交付；deferred cancelled chat finalization 已提交 Ready PR #575 | 收口 #558/#559/#561/#562/#563 与 #575 的异步 V/R/F；异步结果不阻塞主线 | 依赖 AUDIT-001 Rust daemon 产物及唯一 `RuntimeTaskSweeper::run_once`；可与前序 Ready PR 的异步验证并行 | PR #542..#550/#558..#563/#575；§5.2、§6.2、§29..§37、§45..§51、§62 | 主 agent；独立 V/R/F subagent |
| [~] | AUDIT-006 | Ready PR | 三个 backfill 业务能力、Rust Makefile产物和唯一 production backend image 发布路径已交付；migration operator lifecycle 已接入有界锁等待、信号退出、locked status 与恢复文档 | 异步收口 #555 PostgreSQL/entrypoint finding；不重复创建脱离 backend image 的第二套 backfill release assets | Rust image/package 入口可执行；真实生命周期交异步 V/R/F | PR #518/#519/#520/#523/#555；§6.2、§42 | 主 agent；独立 V/R/F subagent |
| [~] | AUDIT-007 | 进行中 | feature-flag 等局部契约测试已有；T-53 高风险 Go 回归映射索引已提交 Ready PR #576 | 收口 #576 的异步 V/R/F；继续按索引补 API/DB/provider/daemon/security/backfill/CLI contract，标出 Rust 已有证据、待补 contract 与不适用理由；异步结果不阻塞主线 | 依赖 AUDIT-002..006 的能力矩阵；wire/schema/ID 细节转 AUDIT-008 | PR #576；§6.2、§63 | 主 agent；独立 V/R/F subagent |
| [~] | AUDIT-008 | 进行中 | route parity 和部分 wire tests 已有；T-54 已把未接入生产字段的 `cordy-util::Ulid` utility 切到 Go-compatible Crockford codec，并创建 Ready PR #577；T-54A 已把 daemon event ID 生成器切到共享 `ulid` crate，并创建 Ready PR #579；T-54B 已把 realtime/daemon 的全部 ULID 生产调用收口到 `cordy-util`，创建 Ready PR #580；T-54C 已补齐 Go/Rust Redis event envelope 的固定字段、缺失字段和 scope routing contract；T-54D 已把 realtime created_at/heartbeat 切到 Go-compatible RFC3339Nano，并创建 Ready PR #582；T-54E 将 handler/service 已迁移的 RFC3339Nano 输出统一到 `cordy-util` | 收口 #577/#579/#580/#581/#582 异步 V/R/F；T-54E 处理 handler/service timestamp helper centralization；继续完成 JSON/时间/DB/旧数据兼容证据 | utility contract 不是生产兼容或 Go 下线证据；事件切片依赖 AUDIT-002 daemon/realtime 入口 | PR #577/#579/#580/#581/#582/#583；§6.2、§64、§66、§67、§68、§69、§70 | 主 agent；独立 V/R/F subagent |
| [~] | AUDIT-009 | 进行中 | 默认入口、pprof 和 logger 文档已有部分更新；T-55 已将 backfill runbook 切到 Rust 入口并创建 Ready PR #578 | 收口 #578 异步 V/R/F；继续对齐 install/systemd/release/rollback 及剩余运维文档 | 增量文档依赖对应实现；最终退出依赖 AUDIT-001..008 的真实路径 | PR #523/#524/#525/#578；§6.2、§65 | 主 agent；独立 V/R/F subagent |
| [ ] | AUDIT-010 | 待办（最终门） | 尚无 Go 目录可删除 | 仅在 AUDIT-001..009 退出、生产验证通过后，做全仓引用审计并删除全部 Go 源文件 | 严格依赖 AUDIT-001..009 全部退出 | §6.2、§10 | 主 agent；独立 V/R/F subagent |

### 6.1.1 时间顺序执行计划

下面是给主 agent 和后续 Agent 使用的严格先后顺序；上面的详细表仍是唯一事实来源。
这里约束“实现切片何时开始”，不约束异步 verification/review/fix 何时返回。

- `[ ]` = 尚未开始；`[~]` = 已实现/已有 Ready PR，但该切片仍缺退出证据；`[x]` = 该切片的全部退出证据已记录。
- 前一步完成“实现提交、生产入口接线、`git diff --check`、推送和 Ready PR”后，才能开始下一步实现。
- 编译、测试、review 和 fix 在切片交付后异步运行，不作为下一步实现的前置条件；结果必须回写，不能伪报通过。
- `T-xx` 是排程编号，`§xx` 是已有台账章节，不新增 AUDIT ID；已经交付的历史切片不重做。
- 当前 `T-36 / §61`（PR #574）已交付，下一实现游标是 `T-52`：先登记 AUDIT-005 的下一个未闭合 background-worker 缺口，再开始编码。

#### 阶段一：生产基础与发布资产

1. `[~]` `T-01 / §11` AUDIT-001 backend 默认入口
2. `[~]` `T-02 / §15` AUDIT-001 CLI release assets
3. `[~]` `T-03 / §16` AUDIT-001 Desktop 内嵌 Rust CLI
4. `[~]` `T-04 / §17` AUDIT-001 installer/运维入口
5. `[~]` `T-05 / §38` AUDIT-001 tag release verification
6. `[~]` `T-06 / §39` AUDIT-001 self-host image upgrade/rollback
7. `[~]` `T-07 / §40` AUDIT-001 systemd lifecycle
8. `[~]` `T-08 / §41` AUDIT-001 required backend CI gate
9. `[~]` `T-09 / §42` AUDIT-006 migration operator/backfill 发布闭环

#### 阶段二：leaf contract 与 integrations

10. `[~]` `T-10 / §12` AUDIT-003A CPU/cmdline/symbol pprof
11. `[~]` `T-11 / §13` AUDIT-003B logger 基础配置与字段
12. `[~]` `T-12 / §14` AUDIT-003C squad avatar
13. `[~]` `T-13 / §19` AUDIT-003D agent/daemon concurrency
14. `[~]` `T-14 / §43` AUDIT-003A process profiling/metrics replacement
15. `[~]` `T-15 / §44` AUDIT-003B operator log time layout
16. `[~]` `T-16 / §47` AUDIT-003A heap profile/async diagnostics
17. `[~]` `T-17 / §20` AUDIT-004 Lark
18. `[~]` `T-18 / §21` AUDIT-004 WeCom
19. `[~]` `T-19 / §22` AUDIT-004 DingTalk
20. `[~]` `T-20 / §23` AUDIT-004 Slack
21. `[~]` `T-21 / §24` AUDIT-004 Telegram
22. `[~]` `T-22 / §25` AUDIT-004 Composio
23. `[~]` `T-23 / §26` AUDIT-004 VCS
24. `[~]` `T-24 / §27` AUDIT-004 GitHub snapshot
25. `[~]` `T-25 / §28` AUDIT-004 channel media lifecycle

#### 阶段三：API、WS 与 background worker

26. `[~]` `T-26 / §18` AUDIT-002 CLI/daemon control smoke（route parity 424/424 已完成；真实 daemon smoke 仍待收口）
27. `[~]` `T-27 / §52` AUDIT-002 issue-status
28. `[~]` `T-28 / §53` AUDIT-002 issue create admission/order
29. `[~]` `T-29 / §54` AUDIT-002 user WebSocket session
30. `[~]` `T-30 / §55` AUDIT-002 scheduler worker
31. `[~]` `T-31 / §56` AUDIT-002 heartbeat batching
32. `[~]` `T-32 / §57` AUDIT-002 stale-liveness/offline
33. `[~]` `T-33 / §58` AUDIT-002 offline task/reconnect retry
34. `[~]` `T-34 / §59` AUDIT-002 stale dispatched/running/queued cleanup
35. `[~]` `T-35 / §60` AUDIT-002 runtime GC
36. `[~]` `T-36 / §61` AUDIT-002 delegated failure recovery

#### 阶段四：daemon 剩余能力（按既有切片继续）

37. `[~]` `T-37 / §29` AUDIT-005 health/uptime
38. `[~]` `T-38 / §30` AUDIT-005 provider refresh retry
39. `[~]` `T-39 / §31` AUDIT-005 GC metadata
40. `[~]` `T-40 / §32` AUDIT-005 runtime MCP
41. `[~]` `T-41 / §33` AUDIT-005 Remote MCP broker
42. `[~]` `T-42 / §34` AUDIT-005 plugin-hook MCP
43. `[~]` `T-43 / §35` AUDIT-005 local-skills heartbeat
44. `[~]` `T-44 / §36` AUDIT-005 wakeup WS/RPC/control
45. `[~]` `T-45 / §37` AUDIT-005 auto-update/restart handoff
46. `[~]` `T-46 / §45` AUDIT-005 poisoned-session lifecycle
47. `[~]` `T-47 / §46` AUDIT-005 Codex rollout durability
48. `[~]` `T-48 / §48` AUDIT-005 provider demotion/recovery
49. `[~]` `T-49 / §49` AUDIT-005 private task temp lifecycle
50. `[~]` `T-50 / §50` AUDIT-005 wakeup proxy/CONNECT lifecycle
51. `[~]` `T-51 / §51` AUDIT-005 heartbeat HTTP pool recovery
52. `[~]` `T-52 / §62` AUDIT-005 deferred cancelled chat finalization（cancel-ack 与 sweeper fallback；PR #575 Ready，待异步退出证据）

#### 阶段五：最终兼容与退休门

53. `[~]` `T-53 / §63` AUDIT-007 Go 测试契约映射（PR #576 Ready，待异步退出证据）
54. `[~]` `T-54 / §64` AUDIT-008 UUID/ULID utility wire serialization（PR #577，待异步退出证据）
55. `[~]` `T-54A / §66` AUDIT-008 daemon event ID generator cutover（PR #579 Ready，待异步退出证据）
56. `[~]` `T-54B / §67` AUDIT-008 realtime/daemon ULID generator centralization（Ready PR #580，待异步退出证据）
57. `[~]` `T-54C / §68` AUDIT-008 Redis event envelope cross-language contract（Ready PR #581，待异步退出证据）
58. `[~]` `T-54D / §69` AUDIT-008 realtime RFC3339Nano timestamp wire compatibility（Ready PR #582，待异步退出证据）
59. `[ ]` `T-54E / §70` AUDIT-008 handler/service RFC3339Nano helper centralization（下一条 Rust 契约切片）
60. `[~]` `T-55 / §65` AUDIT-009 backfill runbook Rust 入口对齐（Ready PR #578，待异步退出证据）
61. `[ ]` `T-56` AUDIT-010 Go 源码退休

每一步都按同一个交付门执行：登记缺口 → 实现完整业务契约 → 接入唯一 Rust 生产入口 →
运行机械检查 → 提交/推送 → 创建 Ready PR → 记录异步 V/R/F → 收齐退出证据后才把该步改为 `[x]`。

执行规则：一次只从“下一动作”选择一个不重叠的主线业务切片；切片完成后
立即提交、推送并创建 Ready PR，同时回写本表。verification/review/fix 可以
并行运行；主 agent 只从依赖已满足的项继续选择，不需要等待异步结果。切片大小
按完整生产能力/完整契约及其退出条件决定，不按行数决定；禁止仅为补测试、allow、
说明或制造小 PR 而拆开同一能力。

### 6.1.2 细粒度验收清单

下面把每个能力轨道拆成“可独立验收”的子项，方便直观看剩余工作。它不是第二套
ID，也不要求一个子项单独创建 PR；每项都应能回指本台账已有的 PR 或章节证据。
一个主 ID 只有全部子项和退出证据齐全后才能标 `[x]`。`[~]` 表示实现/PR 已交付，
但仍缺异步验证、生产 smoke、review/fix 或回滚证据；`[ ]` 表示尚未开始或尚无足够证据。

#### AUDIT-001 — 默认生产入口与发布切换

- `[~]` Rust server 默认构建、启动、health/ready 与信号退出
- `[~]` Rust CLI/cordy 默认构建、`--help`/`--version` 与退出码
- `[~]` Rust migrate up/down/status、锁等待、取消和恢复入口
- `[~]` Docker backend image、entrypoint、兼容 binary 名称与启动顺序
- `[~]` Helm backend、CI required gate 与 tag release workflow
- `[~]` Desktop 内嵌 CLI、installer/Homebrew/release asset 链
- `[~]` self-host exact-image upgrade、旧版本回滚和 pinned/custom ref
- `[ ]` 新鲜环境的完整 build/check、启动、升级和回滚验收闭环

#### AUDIT-002 — API、WS、CLI 与 background worker 契约

- `[x]` 424/424 route method/path parity
- `[~]` CLI command tree、daemon control、health 与退出码 smoke
- `[~]` issue-status list/create/update/archive/reorder 的权限、事务、错误 JSON 和 event 顺序
- `[~]` issue create duplicate admission、position ordering、autopilot guard 与并发竞态
- `[~]` user WebSocket auth、membership、scope ownership、subscribe/ping/disconnect 隔离
- `[~]` scheduler distributed claim、lease、retry、stale-owner 和 reentry
- `[~]` heartbeat batching、offline fallback、coalesce、flush 和 shutdown
- `[~]` stale runtime liveness/offline task recovery
- `[~]` stale dispatched/running 与 queued TTL cleanup
- `[~]` runtime GC transactional lifecycle
- `[ ]` 将上述切片的真实 DB/loopback/daemon smoke 结果全部收口并形成退出矩阵

#### AUDIT-003 — leaf contract

- `[~]` CPU/cmdline/symbol pprof loopback listener 与公开路由隔离
- `[~]` heap profile、async runtime diagnostics、Linux/non-Linux fail-closed
- `[~]` server/migrate/backfill/daemon logger level、TTY、component、request attrs
- `[~]` 本地时间布局、rotating sink、timezone/DST 与 operator 文档
- `[~]` squad avatar 读写、私有对象签名、扩展名和归属校验
- `[~]` agent 每实体并发上限与 daemon 进程级 slot pool 生命周期
- `[ ]` leaf 的真实产物、开销、对象存储和跨平台生产证据全部收口

#### AUDIT-004 — integrations 生产矩阵

- `[~]` Lark production credentials、HTTP/WS、shutdown 和 fail-closed
- `[~]` WeCom credentials、官方 endpoint、relay/media 和 shutdown
- `[~]` DingTalk credentials、inbound/outbound/media 和 shutdown
- `[~]` Slack credentials、Socket Mode、typing/outbound 和 shutdown
- `[~]` Telegram credentials、long-poll、media/outbound 和 shutdown
- `[~]` Composio API/state secret、HTTP 与 task overlay wiring
- `[~]` VCS integration flag、SecretBox、connect/rotate/webhook
- `[~]` GitHub snapshot credentials、GraphQL/HTTP 和非法凭证降级
- `[~]` channel-engine lease/media、supervisor、retry 与 cancellation
- `[ ]` 每个 provider 的真实凭证 smoke、坏凭证/网络失败、回滚和不可测原因矩阵

#### AUDIT-005 — daemon 完整能力验收

- `[~]` health/uptime、registration/reconcile 与 provider refresh/retry
- `[~]` runtime registry、GC metadata 和 runtime identity lifecycle
- `[~]` local-skills heartbeat list/import/report
- `[~]` wakeup WebSocket/RPC、control consumer、proxy/CONNECT 与取消
- `[~]` task execution、poisoned-session retry、session rollout durability
- `[~]` confirmed provider demotion、hold/barrier 与恢复
- `[~]` private task temp、权限、长路径和 success/failure/cancel cleanup
- `[~]` auto-update、server update、restart handoff 与 rollback
- `[~]` heartbeat HTTP pool stale eviction 与新连接恢复
- `[ ]` 真实 daemon 进程的 registration→claim→execute→reconcile→shutdown 全链路
- `[ ]` server/daemon/Windows/Linux/Docker 产物与资源开销证据

#### AUDIT-006 — migration/backfill 发布闭环

- `[~]` Rust migration runner 的 up/down/status 与 advisory lock
- `[~]` timeout、SIGINT/SIGTERM、退出码和 pending/recovery 行为
- `[~]` 三个 backfill bin 的构建、参数、日志和失败语义
- `[~]` Makefile、Docker image、entrypoint、CI 和 release packaging 一致
- `[ ]` 新鲜 PostgreSQL、镜像启动和 operator recovery 的真实演练

#### AUDIT-007 — Go 测试契约映射

- `[ ]` API/auth/permission/error JSON 回归索引
- `[ ]` DB transaction/locking/rollback 回归索引
- `[ ]` provider/integration/fail-closed 回归索引
- `[ ]` daemon/task lifecycle 与 concurrency 回归索引
- `[ ]` security boundary、backfill、CLI/exit-code 回归索引
- `[ ]` 每项标记 Rust 已有测试、需新增测试或不适用理由

#### AUDIT-008 — wire/schema/ID 兼容性

- `[ ]` JSON null/empty、错误码和错误 envelope
- `[ ]` 时间、timezone/DST 和 RFC3339 精度
- `[ ]` UUID/ULID 序列化、解析和旧数据读取
- `[ ]` Redis key/channel 与跨进程 event envelope
- `[ ]` DB nullable/enum、旧 schema 和迁移后读取
- `[ ]` golden vector、round-trip 和跨语言 fixture

#### AUDIT-009 — 运维与文档切换

- `[~]` README/install 与 Rust binary 名称、参数和默认入口
- `[~]` Helm/Docker/self-host 启动、升级和回滚文档
- `[~]` systemd unit、linger、stop/start 和失败清理文档
- `[~]` release/compatibility/rollback 说明与实际 asset/ref
- `[~]` pprof/metrics/logger/operator troubleshooting 文档
- `[ ]` 文档逐项通过新鲜产物和真实命令复核

#### AUDIT-010 — Go 源码退休最终门

- `[ ]` 全仓 Go import/call/reference、生成代码和脚本引用审计
- `[ ]` 默认 build/deploy/release/installer 不再需要 Go
- `[ ]` 生产 smoke、升级/回滚演练和完整验证通过
- `[ ]` 删除 server/cmd、internal、pkg、tests、go.mod/go.sum 及剩余 Go assets

### 6.2 任务范围与退出证据

以下条目是执行台账的详细定义；它们规定每个任务何时真正完成。

#### P0

##### [~] AUDIT-001：Rust 默认生产入口切换

- 范围：Makefile 的 server/cordy/build/migrate/test/dev/check、scripts/check.sh、scripts/dev.sh、Dockerfile、docker/entrypoint.sh、Helm backend、systemd/install/release 入口。
- 现状证据：默认后端和容器入口已由 PR #523 切到 Rust；PR #527 将 CLI release 资产链改为 Rust；PR #528 将 Desktop packaging 从 Go 源码嵌入改为按目标构建 Rust CLI；本切片对齐 installer/手工运维文档。
- 交付：默认产出 cordy-server、cordy-cli、cordy-migrate 及三个 Rust backfill；Rust release、Desktop 和 installer 保留兼容的 binary/asset 名称或有明确迁移说明；启动、迁移、信号、退出码和回滚路径可演练。
- 退出证据：新鲜 worktree 的 build/check、镜像启动 health/ready、migrate up/down/status、CLI --help/version、回滚演练均以 Rust 产物为准。
- owner：主 agent 迁移/接线；Volta 异步 review/fix。

##### [~] AUDIT-002：生产行为与完整契约 smoke

- 范围：route parity 之外的认证、权限、事务、错误码/JSON、WS、realtime、background worker、CLI 退出码、daemon control/health。
- 交付：按业务能力建立可执行矩阵；每项标记 Go contract、Rust entry、生产是否切换、Go 是否可删。
- 退出证据：关键 API/WS/CLI/daemon smoke 在 Rust 默认产物上通过，并有失败路径和回滚记录。
- owner：主 agent 负责迁移与机械验证；review 与 fix 交给两个独立 subagent。

##### [~] AUDIT-003：未闭合 leaf contract（pprof、logger、avatar、concurrency）

- pprof：Rust `cordy-server::profiling` 已在 127.0.0.1:6060 启动独立 listener，迁移 CPU profile、index、cmdline 和 symbol；heap/trace 尚未等价，必须继续迁移或明确替代并保持运维文档诚实。
- logger：Go 的 LOG_LEVEL、TTY color、component、request_id/user_id/client metadata 已在 Rust 入口对账；Rust 保留 RUST_LOG 作为未设置 LOG_LEVEL 时的兼容回退，默认级别与 Go 一样是 debug。
- Squad avatar：Rust `cordy-handler::squad` 已把响应接到现有 `avatar::resolve_url`，创建/更新接到 `avatar::accept_url`；这复用了已有 HMAC、存储归属和 standalone-image 发布校验，不重复实现 signer。私有对象的 squad 读写契约已迁移并接线；avatar endpoint 的下载策略与剩余 Go 退休仍需整体生产验证。
- agentconfig：Go 默认 max concurrent tasks 为 6、合法范围 1..50；Rust 由 `cordy-config::agent_concurrency` 统一 CLI/API contract。daemon 的默认 20 是独立的进程级 task slot pool，不是 agent 默认值；它从 `CORDY_DAEMON_MAX_CONCURRENT_TASKS`/CLI override 进入 `cordy-daemon::task_execution`，要求大于 0。
- 退出证据：每个 leaf 明确为“Rust 迁移并接线”“已由现有模块吸收”或“仍需迁移”，并有对应测试/生产路径。
- owner：主 agent 负责真正迁移；Volta 负责 review/fix。

##### [~] AUDIT-004：integrations 生产配置矩阵

- provider：Lark、WeCom、DingTalk、Slack、Telegram、Composio、VCS、GitHub snapshot，以及 channel-engine/lease/media。
- 正向场景：有效凭证、真实 outbound、inbound/session 路由、media、重试和 shutdown。
- 负向场景：缺凭证、坏凭证、绑定缺失、网络失败必须可观测且 fail-closed；测试 Stub/Noop 不能被有效生产配置误选。
- 退出证据：每个 provider 有 Rust entry、配置开关、最小 smoke 或明确的不可测原因和回滚策略。
- owner：主 agent 负责迁移/生产接线；Volta 异步处理安全和回归修复。

##### [~] AUDIT-005：daemon 完整能力验收

- 范围：control/health、registration、reconcile、runtime registry、provider refresh、task execution、wakeup/WS RPC、GC、repo cache、local skills、auto update、MCP broker。
- 现状：Rust production stack 已存在，但有 43 条 S9-integration 标记、28 个相关文件，且 crate 顶层仍写着“awaiting daemon wiring”。
- 交付：按真实调用关系逐项验收并移除已无意义的 seam/allow；若某 seam 是真实依赖，补真实 trait/entry，不做仅为清注释的 PR。
- 退出证据：daemon 生产进程可启动、控制面可用、task/provider/GC/reconcile 生命周期通过；不再依赖 Go daemon。
- owner：主 agent 迁移/接线；Volta review/fix。

##### [~] AUDIT-006：migration 与 backfill 发布闭环

- Rust 已有 cordy-migrate 和 backfill_task_usage_hourly、backfill_issue_last_activity、backfill_codex_usage_cache 三个 bin；对应业务切片已在 PR #518、#519、#520。
- 当前 Dockerfile 只构建/复制两个旧 backfill，Makefile build 没有三个 Rust backfill 的默认产物，CI 仍以 Go migrate 为主验证之一。
- 交付：迁移 hooks、advisory lock、取消/超时、状态/退出码、三个 backfill 的 image/Makefile/release packaging 一致。
- 退出证据：新镜像只需 Rust migration/backfill 产物即可完成升级和运维恢复。
- owner：主 agent；Volta 异步 review/fix。

#### P1

##### [ ] AUDIT-007：Go 测试契约映射

- 不按 807 个 Go test 文件机械复制。
- 先按 API、DB transaction、provider、daemon lifecycle、security boundary、backfill、CLI contract 建索引。
- 每个高风险 Go 回归用例标记 Rust 已有测试、需新增测试、或不适用及理由。
- 退出证据：关键 contract 有 Rust 可执行测试；测试失败由 Volta 处理，主 agent 不代做修复。

##### [ ] AUDIT-008：wire/schema/ID 兼容性

- 对齐 JSON null/empty、时间格式、UUID/ULID、Redis key/channel、DB nullable/enum、错误码和事件 envelope。
- cordy-util 当前明确留下 ULID TODO：wrapper 的 serde 仍输出 UUID hyphenated string，而 Go wire contract 使用 26 字符 Crockford ULID；必须在删除 Go 前完成或证明所有当前路径不使用该 wrapper。
- 退出证据：golden vectors/round-trip/旧数据读取和跨语言事件 fixture 通过。

##### [~] AUDIT-009：运维与文档切换

- 更新 SELF_HOSTING_ADVANCED.md、Helm 注释、README/install/systemd、release 说明、pprof/metrics/rollback 文档。
- 文档中的 go run ./cmd/...、go tool pprof 和 binary 名称必须与实际 Rust 产物一致。
- 只在 AUDIT-001 的默认入口确定后落地，避免先写一套与产物不一致的文档。

##### [ ] AUDIT-010：Go 源码退休门槛

- 建立最终删除清单：server/cmd、internal、pkg、Go generated/query、Go tests/testutil、go.mod/go.sum、Docker/CI/release 引用。
- 删除前必须通过全仓 import/call/reference 搜索、Rust production build、部署 smoke、回滚演练和完整测试。
- 只有整个项目完成迁移、生产验证通过并删除全部 Go source 后，goal 才能结束。

## 7. 已落地但禁止重复派发的工作

以下项目不能因为 Rust 文件布局不同而重新拆成重复 PR：

- route contract：424/424 已通过；后续只补行为/生产 smoke；
- storage：attachment_storage、S3/local/CloudFront 及 Rust server 注入已存在；只补真实缺口；
- feature flags：Rust service 已有契约覆盖，包含 9/9 顺序、variant、env、YAML 和跨语言分桶检查；
- entitlement：Rust provider、cache/policy 和 cordy-server 注入已存在；
- selfexec：已吸收到 daemon update_executor，除非调用路径审计发现遗漏；
- agent provider：Rust registry/backend 已覆盖主要 provider，不按 Go 文件逐个复制；
- Lark/WeCom 等 channel：当前 Rust crate 和生产 wiring 已有，先做 AUDIT-004 矩阵；
- backfill #518/#519/#520：能力切片已交付，当前缺口是 packaging/cutover，不重复实现业务逻辑。

## 8. 审计验证记录

| 检查 | 结果 | 说明 |
| --- | --- | --- |
| source inventory | 通过 | Go 1,445/807 tests/638 non-test；Rust 527 source + 5 external test files |
| route parity | 通过 | 424 contract、424 Rust、missing 0、extra 0 |
| cargo fmt --all -- --check | 未通过 | 基线 Rust 多文件已有 rustfmt diff；审计分支未修改实现 |
| cargo test --workspace --all-targets --locked | 未通过 | cordy-daemon hermes.rs/openclaw.rs 有 7 个编译错误；交给异步 fix 队列 |
| go build ./... | 未执行 | 审计环境 shell 没有 go 命令（zsh: command not found: go），不是把它误报为通过 |
| old migration doc reconciliation | 完成 | 旧 S7/S8/S9/S10 checkbox 与当前 Rust source/route/assembly 不完全一致，已降级为历史记录 |

审计环境备注：

- cargo 位于 /home/ubuntu/.cargo/bin/cargo；shell 默认 PATH 未暴露 cargo，验证时使用绝对路径。
- 之前尝试制作完整临时 archive 时遇到 /tmp 空间不足，已只清理本审计产生的精确临时目录；后续避免复制整个仓库，优先使用 Git tree、grep 和直接 worktree。
- 上述环境问题和基线编译错误必须保留在验证记录中，不能通过修改审计文档伪装成绿灯。

## 9. 下一步选择规则

本审计 PR 不实现业务代码。审计文档提交后，下一业务切片从 P0 选择，优先 AUDIT-001 或 AUDIT-003 中已被 source 证据确认的真实缺口。选择后：

1. 为完整 Go 能力或生产契约建立独立 branch/worktree；
2. 主 agent 只迁移和接线；
3. 立即把 review/fix 派给当前 subagent，继续下一块，不等待；
4. 机械验证完成后立即提交、推送并创建 Ready PR；
5. PR body 必须写明 Go 能力、Rust 入口、生产是否切换、Go 是否可下线、验证结果和异步 subagent 状态；
6. 在本清单中更新证据，不用旧文档 checkbox 代替当前状态。

## 10. 审计 PR 自身的交付声明

- 新增/迁移的 Go 能力：无；本 PR 是独立的全局盘点与执行控制面。
- Rust 入口：记录并核对 cordy-server、cordy-cli、cordy-migrate、cordy-daemon 及 backfill bins。
- 生产路径是否已切换：否；审计证明默认 Makefile/Docker/release 仍是 Go，切换列为 AUDIT-001。
- Go 代码是否可以下线：否。
- 当前验证：route parity 通过；Rust fmt/test 有基线失败；Go build 因环境无 go 未执行，详见第 8 节。
- 异步 subagent：Volta 继续处理既有 PR #518/#519/#520 的 review/fix；其结果不阻断本审计文档或后续迁移。

结束条件仍是：全项目完成 Go→Rust、默认生产路径和发布链路切换、生产验证通过，并删除全部 Go 源文件；在此之前不得结束 goal。

## 11. [~] AUDIT-001 执行更新

后续切片 `codex/cord-188-rust-production-cutover` 已开始收口 AUDIT-001 的后端生产边界：

- Rust 入口：`Makefile` 的 `server`、`cli`/`cordy`、`build`、`test`、`migrate-up/down`，以及 `scripts/dev.sh`、`scripts/check.sh`；统一通过 `server-rs` 的 `cordy-server`、`cordy`、`cordy-migrate`。
- 容器入口：`Dockerfile` 不再构建 Go runtime binary，改为构建 Rust server、CLI、migration runner 和三个 Rust backfill，并继续提供 `server`/`cordy`/`migrate` 兼容产物名；`docker/entrypoint.sh` 无需改名即可继续执行迁移后启动。
- CI/Helm：部署构建与迁移验证改用 Rust，并新增生产镜像构建门；Helm 的 backend 注释改为 Rust 入口事实。
- 生产路径状态：PR #523 覆盖本地默认入口、自托管镜像和 CI 部署验证；PR #527 将 CLI release workflow 和 Homebrew formula 输入改为 Rust；本切片将 Desktop 内嵌 CLI 的 smoke/release 构建改为 Rust。install/systemd 全链路和回滚目标仍未整体闭合，故 AUDIT-001 尚未完成。
- Go 是否可下线：否。Go compatibility build/test、CLI release/install、回滚目标和剩余 leaf contract 仍在清单中。
- 验证状态：shell 语法、`git diff --check`、Makefile entrypoint/build contract 已通过；Helm 未执行（审计环境无 `helm`），Docker 构建和 Rust workspace 编译继续按本切片记录，不以环境缺失冒充通过。

## 12. [~] AUDIT-003 执行更新：Rust CPU pprof

后续切片 `codex/cord-189-pprof-rust` 收口了 `internal/profiling` 的 CPU
profile 能力和管理端口边界：

- Rust 入口：`cordy-server::profiling::serve` 在固定的
  `127.0.0.1:6060` 独立 listener 上提供 `/debug/pprof/`、`cmdline`、`profile`
  和 `symbol`；CPU profile 使用 pprof protobuf，并保持 gzip 响应格式。
- 生产路径状态：Rust server 在主 API listener 启动后无条件启动该 loopback
  listener；它没有挂到公开 API router，heap 与 runtime trace 尚未宣称可用。
- Go 是否可下线：否。Go `internal/profiling` 的 heap/trace 等未闭合能力、logger、avatar
  和 concurrency 仍在 AUDIT-003；其余 Go 源码退休门槛也未满足。
- 文档状态：`SELF_HOSTING_ADVANCED.md` 已改为 Rust CPU profile 命令，并明确记录
  heap/trace 的当前限制，避免把未实现能力写成生产承诺。
- 验证状态：Cargo metadata、锁文件一致性、Rustfmt（触及文件）和 `git diff --check`
  通过；workspace check 仍被基线 `cordy-slack` 1 个 exhaustiveness 错误及
  `cordy-daemon` 7 个既有编译错误阻断，已交给 Volta 异步处理。

## 13. [~] AUDIT-003 执行更新：Rust logger contract

后续切片 `codex/cord-190-logger-rust` 收口了 Go `internal/logger` 的进程级
配置和请求属性传播，不另建 logger 框架：

- Rust 入口：`cordy-util::logging` 集中解析 `LOG_LEVEL`、保留未设置时的
  `RUST_LOG` 回退、实现 Go 的 debug 默认和 `warn`/`warning` 别名，并集中提供
  `stderr` TTY 判定；`cordy-server`、`cordy-migrate` 及三个 backfill 使用同一
  初始化路径，daemon 的轮转 writer 继续由 `cordy-daemon::bootstrap` 管理。
- component/请求属性：daemon production stack 运行在
  `component=daemon` span 中；HTTP request middleware 把非空的
  `request_id`、经 JWT 独立验证的 `user_id` 和 `client_*` 元数据放入 handler
  span，使 handler 日志继承 Go `RequestAttrs` 的维度。未经验证的
  `X-User-ID` 不会成为身份归因。
- 生产路径状态：当前 Rust 默认 server、migration/backfill 和 daemon 路径已
  使用该契约；非 TTY 明确关闭 ANSI，daemon 文件 sink 仍保持无色，TTY 前台
  daemon 保持彩色输出。
- Go 是否可下线：否。Go release/install 路径、剩余 leaf contract、pprof
  heap/trace 以及最终全仓 Go 退休门槛仍未完成；tracing 默认的时间文本布局
  也没有被宣称为 Go tint 的字节级兼容契约。
- 验证状态：`cordy-util` 22 个测试通过，其中新增 logger precedence/default
  矩阵 3 个通过；受影响包的 workspace 检查继续记录既有 Slack exhaustiveness
  与 daemon 编译基线错误，不因环境/基线问题伪报全绿。

## 14. [~] AUDIT-003 执行更新：Rust squad avatar contract

后续切片 `codex/cord-191-squad-avatar-rust` 收口 Go squad CRUD 对 avatar
对象 URL 的读写契约：

- Go 能力：`server/internal/handler/squad.go` 的 `squadToResponse`、
  `CreateSquad` 和 `UpdateSquad` 会分别解析、规范化并校验 `avatar_url`；Rust
  现在复用同一套已迁移的 avatar capability。
- Rust 入口：`cordy-handler::squad::SquadResponse::from_state` 调用
  `cordy-handler::avatar::resolve_url`；创建和更新调用异步
  `cordy-handler::avatar::accept_url`，保留私有对象签名、存储归属、图片扩展名
  和 standalone upload 校验。
- 生产路径状态：Rust squad list/get/create/update 的 avatar 路径已接线；默认
  Rust server 已由前序 AUDIT-001 切片作为后端生产入口。真实对象存储 provider
  的完整 smoke 矩阵仍在 AUDIT-004。
- Go 是否可下线：否。Go handler、发布/安装链路、其余 leaf contract 和最终全仓
  Go 退休门槛仍未完成。
- 验证状态：触及文件的 `rustfmt --check` 和 `git diff --check` 已通过；仓库级
  `cargo fmt --all -- --check` 仍被既有 `cordy-agent` 格式差异阻断。
  `cargo test --offline -p cordy-handler --lib` 未通过，但只暴露了依赖图中既有的
  `cordy-daemon` 7 个编译错误和 `cordy-slack` 1 个 exhaustiveness 错误，未出现
  squad/avatar 本切片错误。Volta 的 review/fix 继续异步，不作为提交或 PR 前置条件。

## 15. [~] AUDIT-001 执行更新：Rust CLI release assets

后续切片 `codex/cord-192-release-install-rust` 将 CLI 的发布构建从 GoReleaser
切到 Rust，同时保留现有安装器和自更新器已经使用的资产契约：

- Go 能力：原 `.goreleaser.yml` 和 release workflow 构建 `cordy` CLI，生成
  versioned/legacy archive、checksums 和 Homebrew formula；这些发布输入现在由
  Rust CLI 构建矩阵提供。
- Rust 入口：`.github/workflows/release.yml` 的 `rust-cli-build` 在 Linux、
  macOS、Windows 的 amd64/arm64 runner 上执行 `cargo build --locked --release
  -p cordy-cli`，分别生成 `cordy-cli-<version>-<os>-<arch>` 与
  `cordy_<os>_<arch>` 资产；`release` job 生成 `checksums.txt`、GitHub Release
  资产，并用同一份校验值更新 Homebrew formula。
- 生产路径状态：CLI GitHub Release 和 Homebrew 输入已改为 Rust；
  `scripts/install.sh`、`scripts/install.ps1` 的资产名与校验逻辑无需变更，仍能
  消费这两种 Rust archive。Desktop packaging 目前仍在 `bundle-cli.mjs` 中从
  Go 源码生成内嵌 CLI，install/systemd 与回滚演练仍是 AUDIT-001 下一动作。
- Go 是否可下线：否。release verify 仍保留 Go compatibility test 和
  `govulncheck`，Desktop 内嵌 CLI、剩余发布入口和最终全仓 Go 退休门槛未完成。
- 验证状态：release workflow YAML、Homebrew shell 片段和 `bash -n
  scripts/install.sh` 已通过；本地未模拟 GitHub 多平台 runner、Homebrew tap API
  或真实 tag release。`bash scripts/install.test.sh` 在当前环境因 `/tmp` 生成的
  brew stub `Permission denied` 失败，未修改安装器。Rust CLI 构建会经过现有
  daemon 依赖图，当前本地基线仍有 `cordy-daemon` 7 个编译错误，已交 Volta 异步
  处理，不把它们伪报为 release 绿灯。

## 16. [~] AUDIT-001 执行更新：Desktop 内嵌 Rust CLI

后续切片 `codex/cord-193-desktop-cli-rust` 收口 Desktop packaging 仍调用
Go CLI 的生产边界，并沿用 `server-rs` 现有 `cordy-cli` 入口：

- Go 能力：`apps/desktop/scripts/bundle-cli.mjs` 原来按 Desktop 目标设置
  `GOOS/GOARCH`，从 `server/cmd/cordy` 构建并复制 `cordy`/`cordy.exe`；
  现在由 Cargo 按目标 triple 构建同名 Rust CLI，再复制到既有
  `resources/bin/` 位置，保持 Desktop daemon 启动契约不变。
- Rust 入口：`server-rs -p cordy-cli`；macOS、Linux、Windows 的 x64/arm64
  target 映射集中在 `bundle-cli.mjs`。Linux Desktop 使用 musl target，CI 提供
  musl linker；Windows arm64 使用现有 MSVC cross compiler。
- 生产路径状态：`.github/workflows/desktop-smoke.yml` 和 release workflow
  按平台/架构分别准备 Rust target 并调用 `bundle-cli.mjs`；Desktop 内嵌 CLI
  不再从 Go 源码构建。install/systemd、兼容产物和回滚演练仍待 AUDIT-001
  后续切片。
- Go 是否可下线：否。release verify 的 Go compatibility/vulnerability gate、
  安装/回滚链路和最终全仓 Go 退休门槛仍存在。
- 验证状态：`node --check`、`git diff --check`、两个 workflow YAML 解析和
  目标映射纯逻辑检查已通过；目标映射测试文件已加入，但当前环境无
  `pnpm`/Desktop 依赖，Vitest 未运行。针对 Desktop Linux musl target 的本地
  Rust CLI 构建因本机未安装 `x86_64-unknown-linux-musl` target 在依赖编译前
  失败；未模拟 GitHub runner 的 musl/MSVC 构建。默认 Rust CLI 构建的已登记
  `cordy-daemon` 7 个基线编译错误仍不由本切片处理，不把它们伪报为 Desktop
  绿灯。

## 17. [~] AUDIT-001 执行更新：安装与运维入口对齐

后续切片 `codex/cord-194-rust-install-systemd` 对账 install/self-host 运维
入口，修正仍指向 Go 源码的说明，不新增仓库中不存在的 systemd 服务定义：

- Go 能力：`scripts/install.sh`、`scripts/install.ps1` 的 CLI 安装继续消费
  versioned/legacy release asset；self-host installer 通过 Docker Compose 拉取
  backend image；手工运维文档提供构建后二进制和源码 migration 两种路径。
- Rust 入口：release asset 由 PR #527 的 Rust CLI workflow 提供；self-host
  backend image 和 `server/bin/server`、`server/bin/migrate` 兼容产物由
  Rust 构建；`SELF_HOSTING_ADVANCED.md` 的源码 migration 已改为
  `server-rs` 的 `cordy-migrate`。
- 生产路径状态：默认 CLI installer 的资产名、Homebrew 输入和 self-host 镜像
  路径已与 Rust 对齐；手工 Kubernetes/Docker 回滚仍依赖 operator 选择旧的
  Rust image/tag。当前 HEAD 没有已跟踪的 systemd unit，因此 systemd 启动、
  停止和回滚演练仍是明确缺口，不以文档替代实测。
- Go 是否可下线：否。release verify/CI 的 Go compatibility gate、installer
  的真实跨平台 smoke、systemd/回滚演练以及最终全仓 Go 退休门槛仍未完成。
- 验证状态：文档入口静态对账、`bash -n scripts/install.sh` 和相关 diff
  检查已通过；未执行真实发布、Homebrew API、Kubernetes/systemd 或跨平台
  installer smoke。此前 `bash scripts/install.test.sh` 的环境权限失败和
  PowerShell/Helm 工具缺失仍按前述记录保留。

## 18. [~] AUDIT-002 执行更新：CLI 与 daemon control smoke 矩阵

当前切片 `codex/cord-195-cli-daemon-smoke` 先收口不依赖外部服务即可验证的
CLI/daemon 生产契约；它不是把 807 个 Go 测试逐个改写，也不宣称整个
AUDIT-002 已完成。

| 契约 | Go 来源 | Rust 入口 | 当前可执行证据 | 生产/Go 状态 |
| --- | --- | --- | --- | --- |
| CLI 顶层命令树 | `server/cmd/cordy/main.go` 及各 `cmd_*.go` | `cordy-cli::Cli` / `Command` | `top_level_and_daemon_commands_match_go_contract` 对账完整命令集合，包括隐藏但可调用的 `completion`；Rust 复用 clap shell completion 生成器支持 bash/zsh/fish/powershell | Rust CLI 已接线；默认发布已切 Rust；Go 仍保留作兼容验证 |
| CLI 成功输出与失败退出码 | `server/internal/cli/errors.go`、`server/cmd/cordy/main.go` | `cordy-cli/src/main.rs`、`error.rs` | `http_exit_codes_match_go_contract` 与既有 validation message 测试；stdout/stderr 真实 artifact smoke 待执行 | Rust 入口已接线；需在可运行 artifact 上执行 smoke；Go 不可删 |
| daemon profile health/control | `server/internal/daemon` 与 Go CLI daemon commands | `cordy-daemon::control_client`、`production_stack`、`cordy-cli` daemon commands | Rust daemon control parser/health tests；真实进程 smoke 待环境可用后执行 | Rust 内部已接线；默认发布已切 Rust；Go 不可删 |

本切片的退出条件是：命令树无缺口、错误码/输出契约测试可执行、daemon
health/control 的成功和失败路径有记录，并把剩余 API/WS/事务/worker 项目继续
留在本 ID 的下一动作中。验证失败只记录为基线或环境问题并交独立 fix agent，
不由主 agent 在本切片自行修复。

- 初始验证：Go root 与 Rust 测试清单的静态集合对账、默认 Makefile Rust 入口、
  Cargo metadata、触及 Rust 文件的 `rustfmt` 与 `git diff --check` 通过；定向
  `cargo test --locked -p cordy-cli --lib match_go_contract` 在运行本切片测试前，
  被已登记的 `cordy-daemon` `hermes.rs`/`openclaw.rs` 7 个编译错误阻断，不能
  记录为测试通过。Linnaeus 只执行 review（submission
  `01a043b2-de54-7850-9828-03d77f3304aa`）；其结束并关闭后，由新的 fix agent
  接手这 7 个错误，主迁移未等待；后续修复证据如下。
- 交付证据：主切片 `86887169` 已推送到 Ready PR #530；独立 fix 提交
  `25618fcd` 修复 `hermes.rs`/`openclaw.rs` 的 7 个编译错误，后续提交
  `e7ecba1a` 迁移 `completion` 并修复 fresh build 暴露的 CLI 编译/参数冲突。
  Linnaeus 只执行 review（submission `01a043b2-de54-7850-9828-03d77f3304aa`），
  review 与 fix 角色保持隔离。
- 验证状态：清空 Rust target 后的 fresh build 已越过 `cordy-daemon` 与
  `cordy-cli` 编译；低磁盘配置下
  `cargo test --offline --locked -p cordy-cli --lib top_level_and_daemon_commands_match_go_contract`、
  `setup_parser_supports_default_cloud_and_self_host_modes` 和
  `http_exit_codes_match_go_contract` 均通过，触及文件 `rustfmt` 与
  `git diff --check` 通过。默认 debug test link 两次因工作区磁盘耗尽失败；联网
  重试因受限环境 DNS 失败，因此最终使用已锁定的本地 crate 缓存验证。真实 daemon
  进程和 stdout/stderr artifact smoke 仍是本 ID 的明确剩余缺口。

## 19. [~] AUDIT-003D 执行更新：agent 与 daemon concurrency contract

Ready PR #531（`codex/cord-196-agent-concurrency-contract-rust-v2`）收口 Go
`internal/agentconfig/concurrency.go`，并明确它与 daemon 全局槽位不是同一个配置：

- Go 能力：agent create/update/copy 的 `max_concurrent_tasks` 默认 6、合法范围
  1..50；daemon 另以默认 20 限制单进程同时执行的 task 总数。
- Rust 入口：`cordy-config::agent_concurrency` 是 agent contract 的单一来源，
  `cordy-cli` 与 `cordy-handler::agent_api` 共同调用；daemon 继续由
  `cordy-daemon::config::DEFAULT_MAX_CONCURRENT_TASKS` 和
  `cordy-daemon::task_execution` 管理独立的全局 slot pool。
- 生产路径状态：agent CLI/API 已接入共享 contract；Rust daemon production
  assembly 已把独立的正数全局上限传给 task executor。这里不增加第三套抽象或
  把两个不同范围强行合并。
- Go 是否可下线：否。该 leaf contract 已迁移，但 daemon 生命周期 smoke、其余
  AUDIT-001..009 以及最终全仓 Go 退休门槛尚未完成。
- 验证状态：`cordy-config` 的 agent contract 定向测试 2/2 通过；触及 Rust
  文件的固定 stable rustfmt、`git diff --check`、offline locked metadata 通过。
  受影响包的 offline locked check 在本切片代码编译后，被已登记的
  `cordy-slack` 1 个 exhaustiveness 错误和 `cordy-daemon` 7 个
  Hermes/OpenClaw 编译错误阻断；这些基线错误现已由 PR #530 的独立 fix 提交
  `25618fcd`/`e7ecba1a` 修复并传播到本分支。

## 20. [~] AUDIT-004 执行更新：Lark production configuration contract

Ready PR #532（`codex/cord-197-lark-production-contract-rust`）验证 Lark/飞书 production
assembly 的配置选择，不复制 provider 实现或增加第二套 client factory：

- Go 能力：只有配置有效的 `CORDY_LARK_SECRET_KEY` 才启动安装凭证解密、真实
  HTTP/WS transport、backfill、reply、typing、media 与 resolver wiring；缺失或
  非法 key 必须禁用而不是退到可伪成功的 stub。
- Rust 入口：`cordy-server::channel_runtime::configure_lark` 通过
  `channel_secret_box` fail-closed，随后直接构造
  `cordy_lark::http_client::HttpApiClient`、`WsLongConnConnector` 和安装凭证服务；
  production 路径没有选择 `StubApiClient`。
- 生产路径状态：Rust server 的 `ChannelRuntime::start` 调用该 wiring；取消 token、
  outbound shutdown、maintenance join 和 router drain 都受现有 deadline 管理。
- Go 是否可下线：否。Lark 这一配置选择已有 Rust 证据，但真实外部凭证 smoke、
  其他 AUDIT-004 provider 以及最终 AUDIT-001..010 门仍未完成。
- 验证状态：触及文件的 fixed stable rustfmt、`git diff --check`、offline locked
  metadata 通过；Lark stub fail-closed 定向测试 1/1 通过。新增 server production
  gate 测试已开始构建，但在运行测试前因共享磁盘空间耗尽而中止；未把环境失败
  伪报为通过。后续 PR #535 在清理生成缓存并传播独立基线修复后实际运行四个
  provider production gate，Lark/WeCom/DingTalk/Slack 4/4 通过。

## 21. [~] AUDIT-004 执行更新：WeCom production configuration contract

Ready PR #533（`codex/cord-198-wecom-production-contract-rust`）验证 WeCom production
assembly 的配置与 transport 选择，复用现有 resolver、dialer、relay、media 和
shutdown wiring，不增加新的 factory 或生产抽象：

- Go 能力：只有配置有效的 `CORDY_WECOM_SECRET_KEY` 才注册安装凭证解密、真实
  WebSocket transport、inbound/session resolver、outbound、media 与可选 Redis relay；
  缺失或非法 key 必须禁用而不是回退到可伪成功的 fake。
- Rust 入口：`cordy-server::channel_runtime::configure_wecom` 注入
  `SecretboxCredentialsResolver`，并把 `ChannelDeps::dialer` 留空，使既有 factory
  选择 `DefaultDialer` 与官方 `wss://openws.work.weixin.qq.com`；factory 继续拒绝
  缺 resolver、缺 bot_id 和解密失败。
- 生产路径状态：Rust server 的 `ChannelRuntime::start` 调用该 wiring；supervisor
  管理连接租约与重试，取消 token、outbound tasks、relay handles 和 router drain
  都受现有 shutdown deadline 管理。
- Go 是否可下线：否。WeCom 配置选择和 crate 内部契约已有 Rust 证据，但真实外部
  凭证 smoke、其他 AUDIT-004 provider 与最终 AUDIT-001..010 门仍未完成。
- 验证状态：`cordy-wecom --lib` 121/121 通过，覆盖 credential/factory fail-closed、
  subscribe 拒绝、inbound、media、relay、transport deadline 与 cancellation；固定
  stable rustfmt 和 `git diff --check` 通过。首次 server 定向测试于 AWS/Lark
  依赖归档阶段因共享 target 磁盘耗尽中止；清理 9.4 GiB 可再生 Cargo 缓存后的
  低调试信息 fresh rerun 继续到 `cordy-slack`，但被既有
  `EnvelopeKind::Disconnect` 非穷尽匹配阻断。独立 fix agent 随后以 `8fcf8272`
  传播既有修复；PR #535 的组合 server production gate 实际运行 4/4 通过。

## 22. [~] AUDIT-004 执行更新：DingTalk production configuration contract

Ready PR #534（`codex/cord-199-dingtalk-production-contract-rust`）验证 DingTalk
production assembly 的配置与 transport 选择，复用现有 decrypter、token cache、
Stream connector、inbound/outbound、media 和 shutdown wiring，不增加新的生产抽象：

- Go 能力：只有配置有效的 `CORDY_DINGTALK_SECRET_KEY` 才注册安装 AppSecret
  解密、真实 HTTP/Stream transport、access-token cache、reply、ack、media 与
  resolver wiring；缺失或非法 key 必须禁用而不是回退到 fake。
- Rust 入口：`cordy-server::channel_runtime::configure_dingtalk` 直接构造
  `cordy_dingtalk::client::Client::new(None, "")`，默认使用官方
  `https://api.dingtalk.com`，并把同一个 client 注入 factory、outbound、ack、
  replier 和 media；factory 继续拒绝空安装 AppSecret。
- 生产路径状态：Rust server 的 `ChannelRuntime::start` 调用该 wiring；channel
  connector、conversation dispatch、outbound runtime tasks 与 supervisor shutdown
  复用现有生命周期边界。
- Go 是否可下线：否。DingTalk 配置选择和 crate 内部契约已有 Rust 证据，但真实
  外部凭证 smoke、其他 AUDIT-004 provider 与最终 AUDIT-001..010 门仍未完成。
- 验证状态：`cordy-dingtalk --lib` 48/48 通过，覆盖配置解密、安装校验、inbound、
  media guard、Stream frame、dispatch 顺序与 shutdown deadline；固定 stable rustfmt
  和 `git diff --check` 通过。新增 server 定向测试已实际运行构建，但在执行测试前
  被现有 `cordy-slack` `EnvelopeKind::Disconnect` 非穷尽匹配阻断；本切片没有夹带
  修复。独立 fix agent 传播 `8fcf8272` 后，PR #535 的组合 server production gate
  实际运行 4/4 通过。

## 23. [~] AUDIT-004 执行更新：Slack production configuration contract

Ready PR #535（`codex/cord-200-slack-production-contract-rust`）验证 Slack production
assembly 的配置与 transport 选择，复用现有 token decrypter、Socket Mode、slash、
typing、media、outbound 和 shutdown wiring，不增加新的生产抽象：

- Go 能力：只有配置有效的 `CORDY_SLACK_SECRET_KEY` 才注册安装 app/bot token
  解密、真实 Web API/Socket Mode transport、slash command、typing、media、reply 与
  resolver wiring；缺失或非法 key 必须禁用而不是回退到 fake。
- Rust 入口：`cordy-server::channel_runtime::configure_slack` 注入真实 decrypter，
  `cordy_slack::channel::new_slack_factory` 解密安装 token 并构造
  `SlackClient::new`，默认使用官方 `https://slack.com/api/`；factory 拒绝空
  app-level token。
- 生产路径状态：Rust server 的 `ChannelRuntime::start` 调用该 wiring；Socket Mode
  connection、outbound runtime tasks、typing clear 与 supervisor shutdown 复用现有
  cancellation/deadline 边界。
- Go 是否可下线：否。Slack 配置选择和 crate 内部契约已有 Rust 证据，但真实外部
  凭证 smoke、其他 AUDIT-004 provider 与最终 AUDIT-001..010 门仍未完成。
- 验证状态：独立 fix agent 只传播既有 `EnvelopeKind::Disconnect` 修复为
  `8fcf8272`，fresh `cordy-server --no-run` 通过；修复进入堆叠后，
  `cordy-slack --lib` 在具备 loopback bind 权限时 63/63 通过，组合 server
  production configuration gate 实际运行 4/4 通过。沙箱内同一 Slack 测试为
  61 通过、2 个本地 listener 因 `EPERM` 失败，沙箱外原样重跑后两项均通过；固定
  stable rustfmt 与 `git diff --check` 通过。

## 24. [~] AUDIT-004 执行更新：Telegram production configuration contract

Ready PR #536（`codex/cord-201-telegram-production-contract-rust`）直接执行 Telegram
production configure/Registry 路径，复用现有 HandlerState、router、BotApi、media、
outbound 和 shutdown wiring，不增加生产抽象：

- Go 能力：缺失或非法 `CORDY_TELEGRAM_SECRET_KEY` 不注册 Telegram；有效 key
  注入安装 bot token decrypter、真实 Bot API/long-poll transport、typing、reply、
  media、outbound 与 resolver wiring。
- Rust 入口：测试实际调用 `cordy-server::channel_runtime::configure_telegram`，
  断言缺失/非法 key 时 Registry 为空；有效 key 时注册 production factory，并由
  同一 factory 解密 secretbox 安装 token、成功构造 Telegram channel。空
  `api_base` 继续选择 `cordy_telegram::DEFAULT_API_BASE`（官方
  `https://api.telegram.org`）。
- 生产路径状态：Rust server 的 `ChannelRuntime::start` 调用该 wiring；long poll、
  outbound runtime tasks、media resolver 和 supervisor shutdown 复用现有
  cancellation/deadline 边界。
- Go 是否可下线：否。Telegram production selection 已有直接 Rust Registry 证据，
  但真实外部凭证 smoke、其余 AUDIT-004 provider 与最终 AUDIT-001..010 门未完成。
- 验证状态：`cordy-telegram --lib` 43 通过、1 个明确依赖外网的测试 ignored；固定
  stable rustfmt 与 `git diff --check` 通过。新增 server production configure
  测试已启动完整依赖编译/链接，当前尚未返回结果，因此不记录为通过；长编译不阻塞
  Ready PR 或后续主线迁移。

## 25. [~] AUDIT-004 执行更新：Composio production configuration contract

Ready PR #538（`codex/cord-202-composio-production-contract-rust`，implementation
commit `711940c4`，fixer commit `68c903a0`）直接执行 Composio 的 production
HandlerState 选择路径，复用现有
feature flag、ClientBuilder、Service、HTTP state 和 TaskService overlay，不新增
生产 factory 或配置抽象：

- Go 能力：`server/cmd/server/router.go` 只有在 `composio_mcp_apps` flag 开启，且
  API key、state signing secret 和 callback base 完整时，才同时为 HTTP handler
  与每任务 MCP overlay 安装同一个 Composio service；任一 gate 缺失必须 fail-closed。
- Rust 入口：契约测试直接调用
  `HandlerState::new_with_production_dependencies`；缺 API key、缺 state secret、缺
  callback 或 flag 关闭时，`HandlerState::composio` 与 `TaskService::composio` 均为
  `None`。有效配置通过既有 `build_service` 创建真实 `cordy_composio::ClientBuilder`
  client，并同时接入 HTTP 与 task overlay 路径。
- 生产路径状态：Rust server 已使用该 production constructor；默认 client 指向
  `https://backend.composio.dev/api/v3.1`，有效配置不会选择 Stub、Noop 或 Fake。
- Go 是否可下线：否。Composio 选择路径已形成直接 Rust 证据，但 VCS、GitHub
  snapshot、真实外部凭证 smoke 和 AUDIT-001..010 最终门仍未完成。
- 异步状态：独立 verification 在 `c6c6f27e` 上确认 fixed stable rustfmt 和
  `git diff --check` 通过，但精确测试实际运行 1 个并因缺 Tokio context 以 0/1
  失败。reviewer 报告初始化错误被吞、缺 `JWT_SECRET` fallback 覆盖、进程环境未
  串行隔离。独立 fixer commit `68c903a0` 做最小修复；fixer 自测 exact offline/
  locked 为 1 passed、0 failed、382 filtered，targeted rustfmt 与 diff check 通过。
  独立 verification/reviewer 对该 commit 的复验仍在队列，不能用 fixer 自测替代。
  上游 Telegram compile 问题已由 PR #536 fixer commit `62e7f3d5` 修复。

## 26. [~] AUDIT-004 执行更新：VCS production configuration contract

Ready PR #539（`codex/cord-203-vcs-production-contract-rust`，implementation commit
`ee43168b`）直接覆盖 VCS 的 server boot 配置边界，复用现有 Config、SecretBox、
HandlerState、handler 和 `cordy_vcs` provider，不增加新 registry 或兼容路径：

- Go 能力：`server/cmd/server/router.go` 仅在
  `CORDY_VCS_INTEGRATION_ENABLED` 精确为 `true` 时公开自托管 VCS integration；
  `CORDY_VCS_SECRET_KEY` 必须能构造 SecretBox，才能加密 token/webhook secret。
- Rust 入口：`VcsWebhookConfig::from_config` 是 `cordy-server::main` 实际调用的
  boot 选择边界，结果传入 `build_production_router`，再由
  `HandlerState::with_vcs_webhooks` 注入 connect/rotate 与 public webhook 路径。
- 生产路径状态：flag 关闭时路径保持隐藏；flag 开启但 key 缺失或非法时 SecretBox
  为 `None`，写入与 webhook 路径返回 503 而不存明文；有效 32-byte key 构造真实
  `cordy_util::secretbox::SecretBox`。已有连接继续通过 `cordy_vcs::for_kind` 选择
  Forgejo/Gitea/GitLab 实现，不会选择 Stub、Noop 或 Fake。
- Go 是否可下线：否。VCS boot gate 已有直接 Rust 证据，但 provider 外部 HTTP/
  webhook smoke、GitHub snapshot、其余 AUDIT-004 退出证据和 AUDIT-001..010 最终门
  仍未完成。
- 异步状态：verification 与 reviewer 尚未返回，未把编译、测试或格式检查记录为
  通过；fixer 尚未派发。PR 堆叠在 Composio PR #538。

## 27. [~] AUDIT-004 执行更新：GitHub snapshot production configuration contract

Ready PR #540（`codex/cord-204-ghsnapshot-production-contract-rust`，implementation
commit `be02f618`）收口 GHSnapshot 非法凭证会终止 Rust server 的生产差异：

- Go `handler.New` 调用 `ghsnapshot.NewClientFromEnv`；凭证缺失时保持 inert，private
  key 非法时记录 warning 并把 nil client 交给 Manager，server 继续启动。
- Rust 入口：`cordy-server::main` 仍调用真实 `Client::new_from_env`；新增的最小
  `github_snapshot_client` 边界把 `Ok(Some(client))` 原样交给
  `HandlerState::with_github_snapshots`，保留 `Ok(None)`，并把非法 private key 等
  `Err` 记录为 warning 后转成 `None`，不再终止 server。
- 生产路径状态：有效 App ID/RSA key 继续选择默认 `https://api.github.com` 的真实
  reqwest client；Manager、queue、worker、sweeper、root cancellation 与
  `DEFAULT_SHUTDOWN_TIMEOUT` 均复用现有 production assembly。缺失或非法凭证不会
  选择 Stub、Noop 或 Fake，而是让 Manager 保持 inert。
- Go 是否可下线：否。非法凭证降级差异已迁移；真实 GitHub App/GraphQL smoke、
  channel-engine/lease/media 完整生命周期和 AUDIT-001..010 最终门仍未完成。
- 异步状态：verification 与 reviewer 尚未返回，未把编译、测试或格式检查记录为
  通过；fixer 尚未派发。PR 堆叠在 VCS PR #539。

## 28. [~] AUDIT-004 执行更新：channel media production lifecycle contract

Ready PR #541（`codex/cord-205-channel-runtime-lifecycle-contract-rust`，implementation
commit `0e6d5ec1`）直接执行 `ChannelRuntime::start` 使用的 production media worker
边界，复用现有 storage、reconciler、lease store、supervisor 与 shutdown 机制：

- Go 能力：channel media intent reconciler 只在 attachment storage 可用时启动，独立
  于某个 provider；channel runtime 取消时 worker 必须退出。Supervisor 默认使用
  PostgreSQL lease，可选 ready Redis，并以 token-fenced acquire/renew/release 管理
  installation connection。
- Rust 入口：契约测试直接调用 `start_media_reconciler`，证明无 storage 不创建
  worker，真实 `LocalStorage` 经 `ChannelStorage` 创建 worker，`channel_cancel` 后在
  deadline 内正常退出。生产 `ChannelRuntime::start` 同时保留已有 provider registry、
  `RuntimeLeaseStore`、`ChannelSupervisor` 和 metrics wiring。
- 生产路径状态：`build_production_router` 启动 ChannelRuntime；shutdown 依次取消并
  join outbound、supervisor、media、maintenance/relay，最后 drain router。缺 storage
  不会选择 Fake/Noop；无效或不可用 Redis lease 会可观测地禁用 supervisor，而不运行
  无 fencing 的连接。
- Go 是否可下线：否。主线 provider boot 与 media lifecycle 切片已交付，但独立
  verification 尚需实际执行 supervisor/lease 矩阵，并记录真实外部凭证 smoke 或
  明确环境限制和回滚策略；AUDIT-001..010 最终门仍未完成。
- 异步状态：verification 与 reviewer 尚未返回，未把任何编译、测试或格式检查记录
  为通过；fixer 尚未派发。PR 堆叠在 GHSnapshot PR #540。

## 29. [~] AUDIT-005 执行缺口：daemon health uptime wire contract

当前切片选择 `AUDIT-005` 的 control/health 生产调用链。编码前确认的真实差异是：

- Go `server/internal/daemon/health.go` 将截断到整秒的 `time.Duration` 直接格式化，
  因此 65 秒为 `1m5s`、1 小时为 `1h0m0s`。
- Rust `cordy-daemon::production_stack::health_handler` 已由真实
  `DaemonProductionStack::run` 启动并提供 `/health`，但当前把总秒数统一写成
  `65s`、`3600s`。
- Desktop 的既有 `formatUptime` 按 Go 的 `h/m` wire 文本提取摘要；总秒数字符串会
  绕过该兼容路径。CLI 也直接显示此字段。

Ready PR #542（`codex/cord-206-daemon-health-uptime-contract-rust`，gap commit
`a4568beb`，implementation commit `ca77dfa5`）只迁移该 wire 差异：复用现有
production handler 和 `std::time::Duration`，不新增依赖、router、health 类型或测试
专用生产抽象。私有 `format_uptime` 直接由 `health_handler` 调用，边界检查覆盖 0、
秒、分钟和小时的 Go 文本。

- 默认生产路径：Rust CLI/daemon 产物已由上游切换；本 PR 修改
  `DaemonProductionStack::run` 实际启动的 `/health` handler。
- Go 是否可下线：否；其余 daemon control/reconcile/execution/GC/MCP 与最终
  `AUDIT-001..010` 退出缺口继续保留。
- 异步状态：verifier 在 head `95fd873c` 实际通过 fixed-stable rustfmt、PR/worktree
  diff check、locked/offline daemon no-run，并执行定向 uptime test 1/1（439 filtered）。
  未执行真实 HTTP `/health` daemon smoke：当前 production stack 需要完整 listener/config/
  runtime services，不能把私有 handler 单测扩大声明成进程 smoke。review/fix 仍异步；
  上游历史失败继续保留。

## 30. [~] AUDIT-005 执行缺口：provider refresh partial-failure retry

当前切片继续 `AUDIT-005` 的 registration/reconcile 生产调用链。编码前确认的真实差异：

- Go `agentDiscoveryLoop`、`providersMissingRuntimes` 和 `refreshAgentVersions` 从每个
  workspace 已成功接受的 runtime/version 状态决定下一轮目标；某个 register 失败不会
  推进该 workspace，后续 tick 会按 backoff 重试，成功 workspace 不重复发送。
- Rust `ProviderRegistrationSource::begin_builtin_refresh` 当前在任何 workspace register
  之前就更新进程级 `last_builtin_payload`。多 workspace 全部或部分失败后，相同 probe
  payload 在下一轮会返回 `None`，失败 workspace 因此不再收敛。
- Rust 已有 `RuntimeLaunchRegistry`，且只在 `registration_applied` 后按 workspace 更新；
  它已是可复用的 authoritative accepted state，不需要新增第二套 ack/pending registry。

Ready PR #543（`codex/cord-207-daemon-provider-refresh-retry-rust`，gap commit
`7ea19a25`，implementation commit `985e46e1`）只把 refresh 选择改为逐 workspace
比较最后成功应用的 built-in launch 集合，并保留现有 probe、register、demotion
barrier、deregister 和 production reconcile owner。进程级 `last_builtin_payload` 已
删除；`registration_applied` 仍是 accepted launch 状态的唯一推进点。最小检查证明
ws-1 成功应用新版本后只跳过 ws-1，尚未应用的 ws-2 仍需要 refresh。

- 默认生产路径：Rust `DaemonProductionStack` 已启动真实 `provider_refresh_loop`，本 PR
  修改该 owner 调用的 `refresh_builtins_once`；不新增 Stub、Noop 或 Fake。
- Go 是否可下线：否；daemon execution、GC、repo cache、local skills、auto update、
  MCP、真实进程 smoke 和最终 `AUDIT-001..010` 仍未退出。
- 异步状态：verifier 在 head `c1683ce3` 的首次 exact filter 因 shared target 复用了 #542
  stale binary，实际为 0/440，不能算通过；精确清理 105 MiB 可再生 cordy-daemon package
  artifacts 后重跑为 1/1（440 filtered），locked/offline no-run 与 diff check 通过。fixed-
  stable rustfmt 在 `provider_registration.rs` 失败，已交独立 fixer；verifier 只恢复其
  Cargo 命令产生的单 hunk lockfile 顺序副作用，最终 worktree clean。

## 31. [~] AUDIT-005 执行缺口：GC metadata single wire contract

当前切片选择 `AUDIT-005` 的 GC production 调用链。编码前确认的真实差异：

- task completion 已用共享 `execenv::execenv::GcMeta` / `write_gc_meta` 写
  `.gc_meta.json`，但 production `gc_loop` 仍保留一套标为 S9 integration stand-in
  的本地 `GcMeta`、`GcMetaKind` 和 reader；writer/reader 可以独立漂移。
- Go `execenv.GCMetaKind` 是字符串；未知 future kind 会保留 metadata，并由 GC 按
  mtime 降级后继续应用 `local_directory` 保护。共享 Rust enum 当前拒绝未知字符串；
  若直接换用它，reader 会把文件当成无 metadata，绕过该保护并可能整目录清理。
- GC 的真实 `DaemonCoreHost`、client checks、activity reservations、repo cache 和
  production owner 均已接线；不需要新 host、loop 或 registry。

Ready PR #544（`codex/cord-208-daemon-gc-meta-contract-rust`，gap commit
`d2b2ac25`，implementation commit `8a5db875`）让共享 metadata wire 保留 unknown
kind，再让 production GC 和 disk usage 直接读取共享类型，并删除 GC 内重复的类型与
reader。缺失、坏 metadata 仍走既有 mtime fallback；unknown kind 保留原 wire 文本，
再由 GC 降级并继续应用 `local_directory` 的禁止整目录删除保护。

- 默认生产路径：上游 Rust CLI/daemon 已切换；本 PR 修改现有
  `DaemonProductionStack` 所拥有 `gc_loop` 的真实 reader，没有新增第二个 loop、host、
  registry 或 dependency。
- Go 是否可下线：否；AUDIT-005 仍有 reconcile/execution/MCP/local-skills/update 等
  生命周期缺口，且最终 AUDIT-001..010 门尚未收口。
- 异步状态：独立 verification 与 reviewer 尚未返回，fixer 尚未派发。主 agent 只实际
  运行并通过 `git diff --check c1683ce3..8a5db875`；未把编译、测试、rustfmt、静态检查
  或生产验证记录为通过。PR 明确堆叠在 Ready PR #543。
- reviewer 在 head `15e4ec28` 返回一个 P1：实现行为本身核对正确，但新增测试只手造
  metadata 并直接调用 override，没有直接跑
  `read_gc_meta -> unknown dispatch -> local_directory override` 生产决策链，不能支撑
  删除安全声明。该 finding 已交给独立 fixer；尚无修复 SHA 或重新验证结果。
- verifier 在同一 head 如实记录：`git diff --check` 通过；fixed-stable rustfmt 因
  `gc.rs` import ordering 失败；两个 exact filter 均在编译期失败，因为共享类型已改名为
  `GCMetaKind`，测试仍有 7 处 `GcMetaKind`（E0433）。因此没有测试体执行，daemon
  locked/offline no-run 与 disk-usage 验证也被阻断；这些编译/格式问题已交独立 fixer。
- fixer commit `1dcc92db` 已推送到 PR #544：修正全部 7 处 type name 与格式问题，并把
  安全检查改为写真实 unknown/local `.gc_meta.json`、旧 mtime、非零 TTL，再调用 production
  `should_clean_task_dir` 断言只清 managed artifacts。两个 exact 分别 1/1（441 filtered），
  daemon locked/offline no-run、targeted rustfmt 与 diff check 通过；早期 0-test/compile failure
  历史保留。

## 32. [~] AUDIT-005 执行缺口：runtime MCP production merge

当前切片选择 `AUDIT-005` 的任务启动/MCP production 调用链。编码前确认的真实差异：

- Go `runTask` 在构造 execution environment 前调用
  `mergeRuntimeAndAgentMcpConfig`，读取 provider 原生的本机 MCP 配置，再以 agent 保存的
  `mcpServers` 覆盖同名 runtime entry；失败只降级为 agent 配置并留下 warning。
- Rust `runtime_mcp::merge_runtime_and_agent_mcp_config` 已迁移相同 parser、provider 路径、
  JSONC/TOML 兼容和覆盖语义，`ProviderExecutionInputs::effective_mcp_config` 也已存在；但
  production `ProductionProviderAdapter::run_task_inner` 从未调用该函数，因此实际任务只把
  agent 配置交给 `ProviderExecutionPlan`，本机 runtime MCP server 不会进入启动配置。
- 真实 `ProviderExecutionPlan`、execenv sidecar/wrapper 和 provider backend 已接线；不需要
  新 parser、config type、factory 或 production seam。

Ready PR #545（`codex/cord-209-runtime-mcp-production-wiring-rust`，gap commit
`2093b2a1`，implementation commit `1c340cde`）只在 production adapter 构建 plan 前
复用现有 merge，并保持 Go 的 fail-soft warning/fallback。缺少 agent MCP 配置时不设置
override，继续使用 provider 原生 inheritance；存在配置时，合并结果通过已有
`effective_mcp_config` 进入 execenv/provider options。

- 默认生产路径：上游 Rust daemon 已实例化 `ProductionProviderAdapter`；本 PR 修改其
  每个真实任务都会经过的 `run_task_inner`，没有新增 parser、factory、config type、
  dependency 或 test-only seam。
- Go 是否可下线：否；remote MCP broker 与 plugin-hook MCP 仍未进入 Rust production
  task path，AUDIT-005 其余生命周期和最终 AUDIT-001..010 门也未收口。
- 异步状态：独立 verification 与 reviewer 尚未返回，fixer 尚未派发。主 agent 只实际
  运行并通过 `git diff --check 15e4ec28..1c340cde`；未把编译、测试、rustfmt、静态检查
  或生产验证记录为通过。PR 明确堆叠在 Ready PR #544，并如实保留 #544 的 reviewer
  P1 未修复状态。
- reviewer 在 head `b2430a64` 返回一个 P1：调用位置、None/null/unsupported/error fallback、
  plan→execenv/options 数据流、日志安全和 Ponytail 均核对正确，但现有 tests 只分别覆盖
  merge helper 与预填字段传播，未直接执行 production adapter 新调用点。finding 已交
  独立 fixer；尚无 fix SHA 或重新验证结果。
- verifier 在同一 head 通过 provider adapter rustfmt 与 `git diff --check`，并从 source
  确认 assembly 到 production adapter 的调用；runtime MCP exact filter 与 daemon no-run
  均被堆叠基线 #544 的 `GcMetaKind` E0433 阻断，测试体执行数为 0，不能记录为通过。

## 33. [~] AUDIT-005 执行缺口：Remote MCP broker production wiring

当前切片继续 `AUDIT-005` 的任务启动/MCP production 调用链。编码前确认的真实差异：

- Go `runTask` 在 prepare 前调用 `startTaskRemoteMCPBrokers`，用 claim 中的 pinned
  connections 和 daemon token 即时解析凭证、验证 public HTTPS endpoint/tool schema，
  启动 task-local loopback broker；必需连接失败会中止任务，可选连接失败只产生诊断。
- Rust `remote_mcp_broker` 已迁移相同安全代理、provider gate、schema pin、调用/并发/body
  限额、loopback token 和 lifetime cancellation；claim wire 与
  `Client::resolve_remote_mcp_credential` 也已存在，但 production
  `ProductionProviderAdapter::run_task_inner` 从未调用它，故有效 Remote MCP 配置被静默
  忽略。
- PR #545 已把 effective MCP config 接入现有 `ProviderExecutionPlan`；本切片可直接把
  broker overlay 合入该字段，不需要第二个 parser、runner、task owner 或 credential
  client。

Ready PR #546（`codex/cord-210-remote-mcp-production-wiring-rust`，gap commit
`2db66869`，implementation commit `926d8819`）接通 broker startup、异步 credential
resolver、optional diagnostics、fatal failure、config merge 和 task lifetime ownership；
daemon claim wire 删除重复 stand-in，直接复用 `cordy-remotemcp::{Connection, Tool}`。

- 默认生产路径：上游 Rust daemon assembly 对所有 claimed task 使用
  `ProductionProviderAdapter::run_task_inner`；本 PR 在 plan/prepare 前启动真实 secure
  broker，把 overlay 合入 #545 的 effective config，并持有 set 直到 provider execution
  与 environment finalization 完成。有效生产连接构造 `SecureUpstream`；plain HTTP seam
  只存在于测试。
- 直接检查：新增 production `run_task_inner` 检查，必需 Remote MCP connection 配给不兼容
  provider 时必须在执行前失败，不能再静默忽略 claim 配置；broker 内既有检查覆盖 tool
  filtering、credential refresh/revocation、limits、merge 与 drop cancellation。
- Go 是否可下线：否；plugin-hook MCP 尚未进入 Rust production task path，AUDIT-005
  其余生命周期与最终 AUDIT-001..010 门仍未收口。
- 异步状态：verifier 在 head `263ace6b` 通过 `cargo check -p cordy-remotemcp`、
  `git diff --check` 并保持 worktree clean；fixed-stable rustfmt 在
  `provider_adapter.rs`/`remote_mcp_broker.rs` 失败。daemon production exact、broker group
  与 no-run 均被堆叠基线 #544 的 7 处 `GcMetaKind` E0433 阻断，执行数为 0；真实外部
  HTTPS/credential smoke 未运行。verifier 还确认现有测试没有独立的 limit-focused case，
  因而不能支撑 PR 对 limits 的直接覆盖声明。问题已排入独立 fixer；reviewer 异步进行中。
- reviewer 在同一 head 返回两个 P1：其一，合法 JSON scalar root 或非 object
  `mcpServers` 会在 overlay helper 的 `IndexMut` 处 panic，而不是返回 merge error；其二，
  production check 只走 incompatible-provider 的启动前失败，没有直接覆盖有效 secure
  connection 的 credential/discovery、overlay 进入 plan 和 cleanup。两项均已交独立 fixer；
  reviewer 其余核对（wire alias、required/optional、安全 gate、生命周期与 Ponytail）无 finding。

## 34. [~] AUDIT-005 执行缺口：plugin-hook MCP production wiring

当前切片继续 `AUDIT-005` 的任务启动/MCP production 调用链。编码前确认的真实差异：

- Go `runTask` 在 Remote MCP broker 后调用 `startTaskPluginHookMCP`，把 claim-time
  `PluginHookTools` 暴露成 task-local loopback MCP server；tool call 通过 daemon token
  回到 `Client::InvokeAgentPluginHook`，签名 secret 永不进入 daemon。startup 失败只 warning
  并继续任务，overlay merge 失败也只保留既有 MCP 配置。
- Rust `plugin_hook_mcp` 已迁移 tool descriptor、protocol/request limit、60s call timeout、
  loopback path token、server invoker client method和 tests，但
  `ProductionProviderAdapter::run_task_inner` 从未启动它；claim 中的有效 plugin hook tools
  因此被静默忽略。`PluginHookMCPSet` 也只有显式 `close`，尚不能用 Drop 覆盖 production
  task 的所有早退路径。
- #545/#546 已接通 effective config 与 Remote MCP overlay；本切片直接复用相同合并函数、
  `Client::invoke_agent_plugin_hook` 和 task lifetime，不需要第二个 server、client、parser、
  registry 或 runner。

Ready PR #547（`codex/cord-211-plugin-hook-mcp-production-wiring-rust`，gap commit
`3c730a2e`，implementation commit `c267c22f`）把整条 task-local 契约一次接通：production
adapter 构造真实 `Client::invoke_agent_plugin_hook` invoker，启动 loopback server，把 overlay
合入已生效 MCP config，startup/merge 失败按 Go 语义 warning 后继续，并把 server set 持有到
provider execution 与 environment finalization 结束；`Drop` 覆盖所有早退关闭路径。

- 默认生产路径：Rust daemon assembly 的每个 claimed task 都进入
  `ProductionProviderAdapter::run_task_inner`；有效 claim tool 会使用真实 daemon client，
  没有 Stub、Noop 或 Fake 进入有效配置。签名 secret 仍只由 server/client 边界持有。
- 直接检查：新增协议检查通过 overlay URL 实际 POST `tools/call`，证明 production-started
  server 调用 invoker 并返回结果；另有 Drop 检查证明 listener token 随 owner 生命周期关闭。
- Go 是否可下线：否；本契约已迁移，但 AUDIT-005 仍有其余 daemon 生命周期能力和最终
  AUDIT-001..010 门。
- 异步状态：verification 与 reviewer 已分别派发、尚未返回，fixer 尚无本 PR finding。
  主 agent 只实际运行并通过 `git diff --check 263ace6b..c267c22f`；未把编译、测试、rustfmt、
  静态检查或生产验证记录为通过。PR 明确堆叠在 Ready PR #546。
- reviewer 在 head `5e9f049d` 返回三个 P1：duplicate tool 当前被第二项覆盖，违反 Go
  first-wins 路由；继承 #546 的 malformed overlay panic 使 plugin merge fail-soft 不成立；
  直接协议检查注入 test invoker 且未经过 production adapter/真实 Client/effective config，
  不能证明完整 production wiring。三项均已交独立 fixer；其余 client secret 边界、overlay
  precedence、owner/Drop 生命周期、既有协议限制和 Ponytail 核对无 finding。
- verifier 在 branch head `fa1f8c05` 通过 diff check 并确认 assembly→production adapter
  调用链；fixed-stable rustfmt 在两个改动文件失败。两个新增 exact 与 daemon no-run 因该
  堆叠分支尚未传播 #544 的 7 处 E0433 修复而均为 0-test/compile blocked，不能记录通过；
  已交独立 fixer随 #547 findings 一并处理。

## 35. [~] AUDIT-005 执行缺口：local-skills heartbeat list/import/report contract

当前切片选择 `AUDIT-005` 已列出的 local skills 完整生产能力，而不是一个零散 helper：

- Go heartbeat 同时消费 list、旧 singular import 与新 batch imports；每个请求从真实 runtime
  provider 枚举本地/通用/plugin roots 或加载完整 bundle，并把 completed/failed payload 通过
  daemon API 的 5xx retry、4xx fail-closed 与 context cancellation 语义回报。
- Rust 已分别迁移 filesystem discovery、bundle limits、heartbeat fields、client retry 和
  `ProductionProviderAdapter` handler，但没有一条直接检查穿过
  `handle_non_update_heartbeat_actions -> list/import -> Client result endpoint`；删除这些
  production spawn/call 后，现有模块级检查仍可通过。
- `local_skills.rs` 顶层仍声称等待 S9 wiring 并全模块 `allow(dead_code)`，与已经存在的生产调用
  不一致。不能只删注释；本切片必须把 list、singular/batch import 兼容、真实 filesystem
  payload、report path/retry/cancel 和 unknown-runtime drop 作为一个契约收口。

Ready PR #548（`codex/cord-212-local-skills-heartbeat-contract-rust`，gap commit
`ba6fedac`，implementation commit `121a2066`）不重复实现已经存在的生产代码，而是用一条
直接检查穿过 authoritative registry、真实 production adapter、Grok filesystem root、list、
batch 与 singular import、完整 bundle、真实 Client result path 和 HTTP 500 retry；另确认 batch
存在时忽略旧 singular 字段、unknown runtime 不产生请求。过期的 module-level S9 wiring allow
已删除。

- 默认生产路径：Rust heartbeat owner 已调用该 adapter；检查直接调用同一 trait method，删除
  production spawn/report 调用会失败，不再以孤立 helper 测试代替生产证据。
- Stub/Noop/Fake：生产路径无 Stub/Noop/Fake；loopback HTTP 与 temp filesystem 仅属检查。
- Go 是否可下线：此 local-skills 能力已迁移并接线，但 AUDIT-005 其余 daemon 能力和最终
  AUDIT-001..010 门未完成。
- 异步状态：verification 与 reviewer 已派发前状态为 pending，fixer 尚无本 PR finding。主
  agent 仅运行并通过 `git diff --check`，未把编译、测试、rustfmt 或 HTTP 行为记录为通过。
  PR 明确堆叠在 Ready PR #547；#544 fix 尚需传播到这条堆叠分支。
- verifier 在 head `3c831f09` 通过 diff check、保持 worktree clean，并确认默认
  assembly→DaemonCoreHost→services→production adapter 调用链；fixed-stable rustfmt 在
  `provider_adapter.rs` 失败。production heartbeat exact、local-skills group、client retry
  exact 与 daemon no-run 均因未传播 #544 的 7 处 E0433 而 0-run/compile blocked。
- reviewer 在同一 head 返回三个 P1：无锁修改进程级 `GROK_HOME` 不满足并行安全；fixture
  在更新 attempt/发送 HTTP response 前通知且 detached jobs 未确定收口；检查入口从 adapter
  开始，未穿过 daemon heartbeat owner/services，也没有直接覆盖永久 4xx/cancellation。
  finding 与 verifier 的传播/格式问题均已交独立 fixer；其余 batch-first/singular fallback、
  filesystem bundle、registry provider 与 shared Client retry 静态核对无 finding。

## 36. [~] AUDIT-005 执行缺口：wakeup WebSocket / RPC / control consumer lifecycle

当前切片选择 `AUDIT-005` 的 wakeup/WS RPC 与 control lifecycle 作为一条完整能力链：

- Go daemon 用同一条 authenticated WebSocket 发送 heartbeat、接收 heartbeat ack/task hints，
  协商 RPC v1 并执行 WS-first task claim；连接事件再驱动 task poller wakeup、reconcile、
  runtime-gone/profile/workspace refresh 与 heartbeat actions，断线时才安全回退 HTTP。
- Rust `DaemonControl` 已有真实 WebSocket/RPC 检查，`ControlEventConsumer` 也有 synthetic event
  route 检查；但没有直接检查让真实 socket frame 穿过 production `run_daemon_control` 进入
  consumer。删除 transport→event sender 或 production consumer ownership，两组孤立检查仍可过。
- `wakeup.rs`、`wsrpc.rs`、`reconcile.rs` 仍以 S9 awaiting-wiring 为由全模块
  `allow(dead_code)`，而 production stack、manager、consumer 已真实调用它们，台账状态与源码
  自述不一致。

Ready PR #549（`codex/cord-213-daemon-wakeup-control-contract-rust`，gap commit
`4fb9105c`，implementation commit `8d0aafb8`）用真实 loopback WebSocket 和 production
`run_daemon_control` owner 一次穿过 transport、RPC negotiation/correlation、event sender、single
consumer、task wakeup、reconcile、heartbeat lifecycle 与 root cancellation；HTTP heartbeat route
同时存在，避免绕开 production supervisor。wakeup/wsrpc/reconcile 的过期 awaiting-wiring allow
与说明已删除。

- 默认生产路径：`production_stack.rs` 已构造同一 control/consumer/events owner；有效配置使用
  authenticated real client/socket，没有 Stub、Noop 或 Fake dispatcher。
- 直接检查：bearer/capability handshake、heartbeat frame、rpc-v1 ack、task hint、`tasks.claim`
  request/response、Connected/task-specific wakeups、reconcile broadcast、lifecycle heartbeat action
  和 cancellation join 在同一条调用链中断言。
- Go 是否可下线：此 wakeup/control 能力已迁移接线，但 AUDIT-005 其余能力与最终
  AUDIT-001..010 门未完成。
- 异步状态：verification/reviewer 待派发，fixer 尚无本 PR finding。主 agent 仅运行并通过
  `git diff --check`，未把编译、测试、rustfmt 或 socket 行为记为通过。PR 堆叠在 Ready
  PR #548；#544 fix 尚待传播。
- verifier 在 head `582d5c50` 通过 diff check、保持 clean 并确认 production stack 调用链；
  fixed-stable rustfmt 在新测试失败且有 unused `SinkExt` warning。新增 exact、manager real WS、
  wsrpc group 与 daemon no-run 均被未传播 #544 的 7 处 E0433 阻断，执行数为 0。
- reviewer 在同一 head 返回两个 P1：测试自行消费 wakeup 后手工调用 claim，未穿真实
  `TaskExecutionOrchestrator`，故不能声称 task hint→poller→WS claim 同链；HTTP heartbeat route
  无调用断言且 60s randomized initial delay 通常使 supervisor 未执行。两项连同 verifier 问题
  已交独立 fixer；其余 WS auth/RPC correlation/event consumer/cancellation 核对无 finding。

## 37. [~] AUDIT-005 执行缺口：auto-update / server update / restart handoff contract

当前切片选择 `AUDIT-005` 的 auto update 作为一条完整 machine-level 生命周期：

- Go 的 periodic release check、on-disk reload 与 heartbeat pending update 共享同一个 updating
  CAS、claim/task barrier、真实 brew/download executor、result report 和 restart target；Desktop
  必须拒绝自更新，busy/失败必须恢复 claims，成功 handoff 必须保持 barrier 并取消 root。
- Rust 已迁移 `auto_update_loop`、`DaemonCoreHost::handle_server_update`、`UpdateExecutor` 和
  production stack owner，但证据分散在 FakeHost algorithm tests、executor unit tests 与 source
  wiring；没有直接检查从 production heartbeat owner 进入真实 host，确认 Desktop/busy/非法
  target 的 report 与 barrier restore。删除 heartbeat update 分流仍不会破坏现有 auto-update tests。
- `auto_update.rs` 仍全模块 `allow(dead_code)` 并把 CLI helper 标为“直到 CLI crate lands”，而
  Rust CLI、真实 UpdateExecutor 和 production owner 已存在，源码自述落后于默认路径。

Ready PR #550（`codex/cord-214-daemon-auto-update-contract-rust`，gap commit
`0d50fe35`，implementation commit `3e73a2b6`）直接从 production
`DaemonCoreHost::handle_heartbeat_actions` 驱动 pending updates，通过真实 Client result endpoint
证明 Desktop 拒绝、active-task defer、非法 target 进入 concrete `UpdateExecutor` validation，且
每条失败路径都恢复 updating/claim barrier、不取消 root；non-update heartbeat actions 同时执行。
periodic newer-release success/restart 继续由既有 shared `AutoUpdateHost` checks 证明。过期的
auto-update module allow 与 “CLI crate 尚未落地”说明已删除。

- 默认生产路径：production stack 为 heartbeat 与 periodic owner 共享同一 host、activity、
  concrete executor 与 root context；有效配置没有 Stub/Noop updater。
- 安全边界：只新增 `cfg(test)` concrete executor constructor，选择同一实现但避免真实替换测试
  binary；没有新增 runtime trait、installer、barrier、HTTP client 或 restart state machine。
- Go 是否可下线：否；真实 GitHub/Homebrew mutation/rollback、AUDIT-005 其余能力与最终
  AUDIT-001..010 门仍未完成。
- 异步状态：verification/reviewer 待派发，fixer 尚无本 PR finding。主 agent 只运行并通过
  `git diff --check`，未把编译、测试、rustfmt 或 HTTP 行为记录为通过。PR 堆叠在 #549，且
  #544 fix 尚待传播。
- reviewer 在 head `019e6288` 返回两个 P1：新增检查只覆盖 Desktop、busy 与非法 target
  失败，没有 concrete production success/restart、root cancellation 与 barrier hold 证据；
  Desktop/busy 也只断言 `failed`，未核对 Go 的具体错误原因和各分支的 root/updating/barrier
  恢复。finding 已交独立 fixer；reviewer 对 shared production host ownership、`cfg(test)`
  constructor、过期 allow 删除与 Ponytail 无其他 finding。
- verifier 在同一 head 通过两个 diff check；fixed-stable rustfmt 在 `auto_update.rs` 与
  `daemon_core.rs` 失败。production exact、auto-update group、update-executor group 与 daemon
  no-run 都被继承自 #544 的 7 处 `GcMetaKind` E0433 阻断，测试体实际执行数均为 0，不能记录
  为通过；verifier 静态确认 heartbeat 与 periodic update owner 共享同一个
  `Arc<DaemonCoreHost>`。格式、基线传播及 reviewer finding 已合并交给独立 fixer。

## 38. [~] AUDIT-001 执行缺口：tag release verification Go dependency cutover

当前切片选择 `AUDIT-001` 已列出的 release 完整生产链，而不是单独修改一条 CI 命令：

- release asset matrix、Homebrew 包和 backend image 已产出 Rust binary，但 tag workflow 的
  publish 前置 `verify` job 仍安装 Go、运行 Go compatibility tests 和 `govulncheck`；因此即使
  Rust 资产能够构建，缺少 Go toolchain 或 Go 测试失败仍会阻止发布，仓库尚不能声称 release
  链已切到 Rust。
- 现有普通 CI 已有 Rust workspace quality/tests、Rust migration runner、production image 和
  `rustsec/audit-check`，但 tag 是独立发布触发器，不能依赖某个未被 workflow 明确绑定的历史
  branch check 代替发布时验证。
- 本切片将同一个 publish gate 一次切换为：Postgres service、Rust stable/cache、真实
  `cordy-migrate up`、workspace all-target tests、server/CLI/migrate/三个 backfill production
  binary build，以及保持 tag-scoped emergency override 的 RustSec fail-closed audit；同步修改
  release runbook。复用现有 action 和命令，不新增脚本、依赖或并行验证框架。

Ready PR #551（`codex/cord-215-rust-release-verification`，gap commit `39e5ba31`，
implementation commit `6265979d`）已把 publish 前置门整体切到 Rust，并保持 CLI archives、
Homebrew 和 multi-arch backend image 都依赖同一 verify job。

- 默认生产路径：tag workflow 不再安装或执行 Go；Rust migration、workspace tests、全部生产
  package build 与 RustSec 任一失败都会阻止发布。
- 安全边界：RustSec 默认 fail-closed；已有 emergency override 仍必须精确匹配当前 tag，并保留
  warning/runbook，不把移除 `govulncheck` 变成移除依赖漏洞门。
- Go 是否可下线：release workflow 已不依赖 Go，但普通 CI 的 Go compatibility jobs、其余
  AUDIT-001..009 退出和最终 AUDIT-010 删除门尚未收口。
- 异步状态：独立 verification/reviewer 待派发，fixer 尚无本 PR finding。主 agent 只运行并
  通过 `git diff --check`；未把 YAML、Cargo、migration、tests、release build 或 RustSec 记为
  通过。PR 明确堆叠在 Ready PR #550。
- reviewer 在 head `e3131a69` 返回一个 P1 和一个 P2：release `verify` 显式 permissions
  未授予 RustSec action 所需的 `checks: write`，有 advisory/warning 时可能因 Check API 403
  异常阻断发布；runbook 把全部 container assets 说成 Rust binary 过宽，web image 仍是
  Next.js/Node。两项已交独立 fixer；其余 Go toolchain 退出、migration/tests/全部 production
  binary gate、downstream dependency、tag-scoped fail-closed bypass 与 Ponytail 核对无 finding。
- verifier 对未变 workflow 通过 diff、Psych YAML、tag/stable 逻辑、Go command absence 与
  downstream `needs.verify` 静态检查；发现 release-blocking 缺口：`cargo run --locked
  -p cordy-migrate -- up` 因 package 有四个 bin 无法选择 executable，必须显式 `--bin
  cordy-migrate`。workspace tests 与 production release build 仍被继承 #544 的 7 处
  `GcMetaKind` E0433 阻断、实际 0 tests；DB migration、RustSec action 均未在本地完成。
  新缺口和 reviewer finding 已合并交独立 fixer。

## 39. [~] AUDIT-001 执行缺口：self-host Rust image upgrade/rollback ref ownership

当前切片选择 `AUDIT-001` 的 installer→Compose→Rust image 升级/回滚完整生产链：

- Unix/Windows installer 已用 `CORDY_SELFHOST_REF`（默认 latest release tag）checkout 对应
  self-host assets，但 `.env.example` 写入的 `CORDY_IMAGE_TAG=latest` 此后不再同步；因此显式
  `CORDY_SELFHOST_REF=vX.Y.Z` 只切换 Compose 文件，实际 backend/web 仍拉取 `latest`，回滚
  声明不成立，安装时还可能混用旧 assets 与新 Rust image。
- `docker-compose.selfhost.yml` 已有唯一的 `CORDY_IMAGE_TAG` 生产选择边界，不需要第二个
  image resolver、rollback script 或状态文件。installer 应在 checkout 后、pull/up 前把选定
  release ref 写到该字段；`main` fallback 继续选择 `latest`，显式/默认 release tag 则固定同名
  immutable GHCR tag。
- 本切片同时修改 Bash 与 PowerShell installer，并把升级、精确版本固定和回滚命令写入现有
  self-host runbook；不修改数据库迁移语义，也不声称镜像回滚会自动执行 down migration。

Ready PR #552（`codex/cord-216-selfhost-rust-image-rollback`，gap commit `1690be13`，
implementation commit `593cbc0d`）已把 Unix/Windows installer、Git ref checkout、Compose
环境优先级、backend/web exact image tag 和 operator rollback 说明接成同一条生产链。

- 默认生产路径：默认 latest release ref 会写入同名 immutable image tag；`main` fallback 才映射
  `latest`。显式 `CORDY_SELFHOST_REF` 同时控制 checkout 与两张 production image。
- fail-closed：fetch/checkout 失败、非法/过长 image tag 都在 Compose pull/up 前退出；installer
  同时更新 `.env` 和当前进程环境，ambient `CORDY_IMAGE_TAG` 不能覆盖显式 rollback ref。
- Go 是否可下线：此 installer/Compose 路径已选择 Rust backend image，但 systemd、真实启动/
  回滚演练、普通 CI compatibility 和 AUDIT-001..010 最终门仍未完成。
- 异步状态：独立 verification/reviewer 待派发，fixer 尚无本 PR finding。主 agent 只运行并
  通过 `git diff --check`；未把 Bash/PowerShell、Docker、registry 或 rollback 行为记为通过。
  PR 明确堆叠在 Ready PR #551。
- reviewer 在 head `6b7eceed` 返回三个 P1、两个 P2：pull 失败会在失败前污染 checkout 与
  `.env`；无显式 ref 会覆盖既有 pinned/custom tag；ref 在 Git mutation 后才按宽泛 Docker
  syntax 校验而非严格 release/main；新增证据未覆盖 PowerShell、实际双镜像选择和失败恢复；
  PowerShell 重写 `.env` 未固定 PS5-compatible UTF-8。finding 已交独立 fixer。reviewer 对
  Compose 单一 selector/ambient precedence、pull-before-up、DB rollback 警告、Go 声明与
  Ponytail 方向无其他 finding；其 Bash 尝试因 sandbox tmp noexec 失败，pwsh 不可用，均不能
  记录为测试通过。
- verifier 在未变实现上通过两个 diff check、Bash syntax 和 workspace-executable TMPDIR 下
  installer suite 7/7；默认 `/tmp` 的两次运行因 noexec `Permission denied` 失败并保留记录。
  额外只读 harness 的 main→latest、fetch failure-before-Compose、checkout failure-before-Compose
  3/3 通过。环境没有 pwsh，Docker socket 无权限，故 PowerShell、真实 registry pull、真实
  upgrade/rollback 均未执行；这些通过项不解决 reviewer 的 atomic pull/pin/custom/encoding
  findings，已一并交 fixer。

## 40. [~] AUDIT-001 执行缺口：self-host Rust Compose systemd lifecycle

当前切片选择 `AUDIT-001` 已明确列出的 systemd/启动/停止生产生命周期：

- self-host installer 已 clone/pin Rust image 并直接 `docker compose up -d`，container 本身有
  restart policy；但仓库没有受支持的 systemd ownership。operator 无法用安装器声明开机启动、
  统一 stop/disable 或让 unit 在启动前验证 Compose 配置，台账 §17 因此只能写“没有 unit”。
- 发布形态是 Rust backend/web container，不应再新增裸 server tarball、第二套 environment
  parser 或固定 `/opt` 布局。最小完整边界是在现有 Bash installer 增加显式 `--systemd`，生成
  exact `INSTALL_DIR`/Docker path 的 user unit，用同一 `docker-compose.selfhost.yml` 和 `.env`；
  `loginctl linger` 保持注销后/开机 user manager，`systemctl --user enable --now` 获得真实 owner。
- `--stop` 必须优先 disable/stop 已安装 unit，再以现有 Compose down 作为无 unit fallback；
  macOS/Windows 或缺少 systemd user manager 时显式 flag 必须 fail-closed。升级/回滚仍由 §39 的
  installer exact-ref 路径完成，unit 不复制 image-selection 逻辑。
- 本切片只改 Unix installer、现有 installer check 与 self-host runbook；unit 在安装时生成，
  不新增含占位符的模板、systemd crate、root daemon 或第二个 deployment script。

Ready PR #553（`codex/cord-217-selfhost-systemd-rust-lifecycle`，gap commit `fe374402`，
implementation commit `056bee27`）已把 explicit installer flag、user unit generation、Compose
config/start/stop、linger、enable/disable 和现有 exact-image upgrade/rollback 串成单一生命周期。

- 默认生产路径：systemd 是 Linux self-host 的显式 opt-in；unit 使用安装器解析出的 exact
  working directory/Docker executable，并执行同一 `.env`/Compose/Rust backend image，不存在
  第二份配置选择。
- fail-closed：非 Linux、缺 systemctl/loginctl、无 user manager、linger/reload/enable/start 任一
  失败都会显式退出；unit 用 `ExecStartPre ... compose config --quiet` 拒绝非法配置。
- shutdown：`--stop` 先 disable/stop unit，再保留 Compose down，避免容器在 reboot 后复活，
  同时覆盖 unit stop 失败的现有 fallback。
- Go 是否可下线：此 systemd 路径不依赖 Go，但真实 host boot/logout/start/stop/rollback smoke、
  #552 fix、普通 CI compatibility 和最终 AUDIT-001..010 门仍未完成。
- 异步状态：独立 verification/reviewer 待派发，fixer 尚无本 PR finding。主 agent 只运行并
  通过 `git diff --check`；未把 shell、systemd、Docker 或生命周期行为记为通过。PR 明确堆叠
  在 Ready PR #552。
- reviewer 在 head `7f4616c4` 返回一个 P1、两个 P2：systemd 阶段失败可能残留已运行 stack、
  linger 或 enabled-but-failed unit，不满足 fail-closed；linger owner 错误信任可伪造/陈旧的
  `$USER` 而非实际 UID；现有 stub 只证明命令被打印，未覆盖 unit verify、失败清理、Docker
  readiness、已有 unit 升级或真实 stop 生命周期。finding 已交独立 fixer；reviewer 对
  oneshot/restart 基本语义、路径 escaping、#552 finding 继承和 Ponytail 无其他 finding。
- verifier 在 docs-only head `e4d2a04d` 通过两个 diff check、shell syntax、installer suite 8/8
  和补充 stub 4/4；但两次 `systemd-analyze verify` 都确认生成的
  `WorkingDirectory="..."` 被 systemd 249 视为非绝对路径，unit 属 fatal invalid、不会启动。
  当前环境又无可用 user manager/Docker socket，真实 linger、boot/logout 与 Compose 生命周期未执行。
  release-blocking finding 已交独立 fixer，不能把 stub 通过记成真实 systemd 生产通过。

## 41. [~] AUDIT-001 执行缺口：required backend CI Go gate cutover

当前切片选择 `AUDIT-001` 的默认 CI/合并门完整生产契约：

- 普通 CI 已有 Rust quality、workspace tests（含真实 `cordy-migrate up`）、macOS/Windows
  daemon tests、RustSec 和 production image jobs，但 required `backend` 聚合 job 仍把
  `go-backend-tests` 与 `go-vulnerability-scan` 列为硬依赖，并要求两者 success；因此每个
  backend merge 仍必须安装 Go、生成/编译/测试 Go 和运行 `govulncheck`。
- `go-backend-tests` 同时夹带 Helm/Makefile deployment contract；不能为了删 Go 一起丢掉。
  本切片把这部分收敛为轻量 `rust-deployment-contracts` job，继续运行现有 Helm/默认 Rust build
  contract，同时删除重复的 Rust build/migration 和所有 Go-only steps；实际 Rust build、DB
  migration、tests、audit 和 image 分别由已经存在的 authoritative jobs 负责。
- required `backend` 保持原 check name、path-filter skip 语义和 fail-closed aggregation，只从
  Rust deployment/quality/tests/platform/audit/image 六个结果得出 verdict；不新增 workflow、
  action、脚本、dependency 或兼容 shim。

Ready PR #554（`codex/cord-218-rust-required-backend-ci`，gap commit `0eca0e72`，
implementation commit `d607e429`）已经完整删除 required merge gate 的 Go install/build/test/audit
依赖，并保留同名 required `backend` check 与 Helm/Makefile 部署契约。

- 默认生产路径：本能力已切换；backend path 的 merge gate 只从 Rust production image、quality、
  tests、platform、RustSec audit 与 deployment contract 得出 verdict。
- Go 是否可下线：CI required gate 不再依赖 Go，但其他源代码、workflow 与 AUDIT-002..010
  退出条件尚未完成，不能删除 Go。
- 异步状态：独立 verifier/reviewer 待派发，finding 交既有 fixer；主 agent 仅通过
  `git diff --check`，未把 YAML、job graph、path-filter 或 contract scripts 记为通过。
- 已知继承问题：#551 的 migrate bin 选择、#552/#553 review/verification finding 仍在 fixer
  队列，本 PR 不声称这些堆叠问题已解决。
- reviewer 在 head `b480e4ad` 返回一个 P1、两个 P2：仍公开支持的 Go compatibility/rollback
  targets 在源代码删除前失去唯一 build/test/sqlc drift/vulnerability gate；所谓 Rust deployment
  contract 仍运行混合脚本并条件性使用 runner ambient Go；workflow 当前只有 manual trigger，
  所以宣称保留的 PR path-filter skip 分支不可达。finding 已交独立 fixer，必须由其决定保留受控
  legacy gate 或先完整退役 compatibility 边界，主 agent 不代修。
- verifier 在同一 head 通过 diff、Psych YAML、job graph、静态 Go command absence、shell syntax、
  Makefile contract 和正常 changed/unchanged/failure 矩阵；Helm 因本机无 binary 未执行。新增
  fail-closed finding：classifier success 但 backend output 缺失/非法时 aggregate 把它当 no-change
  返回成功。另精确复现 #551 ambiguous migrate command 与 #544 七处 E0433；均已交 fixer，不能
  记录为通过。

## 42. [~] AUDIT-006 执行缺口：Rust migration operator lifecycle

当前切片选择 `AUDIT-006` 的 migration runner 完整运维生命周期，而不是单独补一条命令或测试：

- Rust `cordy-migrate` 已迁移 up/down/status、hook、condition 与 session-pinned advisory lock，三个
  backfill 也已进入 Makefile 和 production image；但 migrate 在等待锁或执行 SQL/hook 时没有
  SIGINT/SIGTERM 取消边界，也没有有界锁等待，滚动升级中一个失联/长跑 runner 可以让后继实例
  无限等待，运维只能依靠进程硬退出。
- `status` 仅检查版本记录，没有复用 migration lock；它可能在另一个 runner 正在逐条执行时返回
  瞬时结果，且当前 runbook 未定义 busy/timeout/interrupted 的非零退出与恢复步骤。
- 本切片在现有 `main.rs`/`runner.rs` 内完成：生产 CLI 的明确 up/down/status contract、统一
  SIGINT/SIGTERM 取消边界、可配置且有默认值的 advisory-lock timeout、status 与写操作
  共享锁边界，以及最小运维文档。复用现有 tokio/sqlx/tokio-util，不新增 crate、框架或兼容层。
- 退出证据：并发 runner 串行；锁超时、信号取消和 pending migration 都 fail nonzero；成功
  up/down/status 保持现有 production entry 与输出；取消或失败后 session lock 可由后继 runner
  获取。编译、数据库并发/信号/恢复验证由独立 verifier 执行，finding 由独立 fixer 处理。

Ready PR #555（`codex/cord-219-rust-migration-operator-lifecycle`，gap commit `3d801ba5`，
implementation commit `0adcc450`）已把上述 CLI、锁、取消、status 与 runbook 作为同一个完整
生产能力交付；修改限于现有 migrate main/runner、self-host runbook 和唯一台账。

- 默认生产路径：Docker/Makefile/CI/release 已有的 Rust migrate entry 保持不变，默认五分钟
  bounded lock 直接作用于生产 binary；CLI flag 优先于环境变量，零值 fail nonzero。
- Go 是否可下线：本 lifecycle 不再需要 Go runner，但仍公开的 compatibility/rollback 入口及
  AUDIT-002..010 退出尚未完成，不能删除 Go。
- 异步状态：independent verifier/reviewer 待派发，finding 交既有 fixer；主 agent 仅运行
  `git diff --check`，未把 compile、CLI、PostgreSQL 并发、signal 或 recovery 记为通过。
- PR 堆叠在 Ready #554；已知 #551 migrate invocation、#544 compile baseline 与 #552..#554
  findings 均保持诚实记录，未在本切片顺手代修。
- 发布入口核对确认 release backend image 由同一 Dockerfile 构建并复制三个 Rust backfill，
  `verify` 也用 `cordy-migrate --bins` 构建它们；这些是服务端数据库运维程序，不属于终端用户
  CLI/Homebrew archive。Ponytail 因此不新增第二套可下载 backfill bundle/checksum 契约。
- reviewer 在 head `d6deeb1b` 返回两个 P1、三个 P2：真实 Docker/Kubernetes PID 1 shell 未向
  migrate child 转发 SIGINT/SIGTERM；timeout 无合理运维上限；fresh DB status 返回裸
  undefined-table；Helm 约 300 秒 startup budget 与默认 300 秒 lock wait 冲突；高风险锁/取消
  逻辑缺直接并发/恢复证据。全部已交独立 fixer，主 agent 不代修。

## 43. [~] AUDIT-003A 执行缺口：Rust process profiling replacement contract

当前切片选择 `AUDIT-003A` 的完整运维诊断契约，而不是伪造 Go heap/runtime trace 格式：

- Go `net/http/pprof` 的 index 隐式提供 Go allocator heap profile，并提供 Go runtime scheduler
  trace；Rust production listener 当前只有 CPU/cmdline/symbol，`trace` 返回 501，`heap` 404，
  metrics registry 的注释也明确承认未接 process collector。
- 已使用的 `prometheus` 依赖原生提供 Linux `ProcessCollector`，可在生产 `/metrics` 暴露 RSS、
  virtual memory、CPU、threads 与 file descriptors。复用它比替换全局 allocator、引入 jemalloc
  profiling 或生成格式看似兼容但没有 allocation stacks 的假 pprof 更小也更真实。
- 本切片将 process collector 接到现有自定义 registry；loopback profiling router 对 Go-only
  heap/trace endpoints 返回稳定 fail-closed retirement response，index 只列真实支持的 CPU
  contract；self-host runbook 给出 memory trend、CPU profile 和 structured tracing 的明确
  Rust 运维替代，不宣称 byte-level pprof heap/runtime-trace equivalence。
- 退出证据：真实 Rust server `/metrics` 包含 process resident/virtual memory/thread/fd metrics，
  public API 仍不暴露 profiling；heap/trace 不会返回 200 或伪 profile；CPU pprof 与日志关联路径
  保持可用。编译、route/metrics/production listener 验证交独立 verifier，finding 交 fixer。

Ready PR #556（`codex/cord-220-rust-process-profiling-contract`，gap commit `2f8851e1`，
implementation commit `f6f8ef32`）已把 process collector、legacy endpoint retirement 和运维说明
作为一个完整诊断能力交付。

- 默认生产路径：非空 `METRICS_ADDR` 选择 production custom registry 与真实 Linux process
  collector；缺失/空值保持 metrics 明确禁用，非法/busy address 走既有 listener error，不回退
  Stub/Noop/Fake。CPU listener 继续只绑定固定 loopback。
- Go 是否可下线：profiling leaf 可在本 PR 异步结果收口后退休；全仓仍依赖其他 AUDIT 退出，
  不能删除 Go。
- 异步状态：verifier/reviewer 待派发，finding 交既有 fixer。主 agent 仅执行 `git diff --check`；
  Cargo resolution/lock、compile、tests、真实 scrape、loopback/public boundary 均未记为通过。
- PR 堆叠在 Ready #555，既有 #544/#551..#555 问题保持在独立 fixer 队列。
- reviewer 在 head `2580eb66` 返回两个 P1、两个 P2：新增 Prometheus feature 未同步
  `Cargo.lock`，所有 `--locked` 路径会失败；process gauges/logs 不能定位 heap allocation stacks
  或 runtime scheduler timing，不能据此关闭 AUDIT-003A；collector 仅 Linux 但文档未限定；测试
  未覆盖真实 METRICS_ADDR listener/scrape/public-router 隔离。台账已立即降回“部分完成”，全部
  finding 交 fixer；410 retirement 不能作为能力等价的证据。

## 44. [~] AUDIT-003B 执行缺口：Rust operator log time layout

当前切片选择 `AUDIT-003B` 已列出的最后一个 logger 生产契约：

- Go `internal/logger` 的 global/component/writer 三条真实入口都固定本地时间布局
  `15:04:05.000`；Rust 已统一 level、TTY、component 和 request fields，但 server、
  migrate/backfills 与 daemon 仍各自使用 tracing-subscriber 默认 RFC3339 timer。
- 这不是要求 Rust 逐字复制 tint 的颜色/字段排版，而是保留 operators、daemon log tail 和既有
  时间窗口关联依赖的本地 `HH:MM:SS.mmm` 前缀。只启用已安装 tracing-subscriber 的 chrono
  timer feature，复用 workspace 已有 chrono，不新增 logger、layer、writer 或 dependency crate。
- 同一个 format constant 由 `cordy-util::logging` 定义，三个 production subscriber 直接调用
  `ChronoLocal`；migration package 已覆盖 migrate 与三个 backfill，不逐 binary 重复接线。
- 退出证据：server、migrate/backfill、foreground daemon 和 rotating daemon sink 都输出本地
  毫秒时间；level/filter/ANSI/writer 行为保持不变。格式、compile、直接输出与各生产入口验证由
  independent verifier 执行，finding 交 independent fixer。

Ready PR #557（`codex/cord-221-rust-log-time-contract`，gap commit `f349b70a`，
implementation commit `0c596cbb`）已用同一个 format constant 和现有 ChronoLocal timer 覆盖
server、migrate/三个 backfill、foreground daemon 与 rotating daemon sink。

- 默认生产路径：全部真实 production subscriber construction sites 已接线；没有新 config，
  LOG_LEVEL/RUST_LOG、ANSI、component/request fields 和 writer/rotation 选择保持原样。
- Stub/Noop/Fake：不存在；subscriber install 继续走既有真实 success/error path。
- Go 是否可下线：logger leaf 可在 #525/#557 异步证据收口后退休；全仓其他 AUDIT 门仍未完成。
- 异步状态：independent verifier/reviewer 待派发，finding 交 fixer；主 agent 仅运行
  `git diff --check`，未把 Cargo、format、compile、output 或 daemon sink 记为通过。

## 45. [~] AUDIT-005 执行缺口：poisoned session retry and retirement lifecycle

当前切片选择 `AUDIT-005` 的 task execution / terminal delivery 完整生产能力，而不是单独接入
一个分类 helper：

- Go `daemon.runTask` 会在已有 session 的 provider resume 明确失败、历史消息不可重放或有限的
  undetectable-backend 场景中，仅在尚未执行 tool 时进行一次 fresh-session retry；retry 前重建
  cold prompt/runtime config，并且绝不把旧 session id 重新嫁接到新结果。
- Go 把两次执行的 token usage 合并；fresh retry 失败或未建立新 session 时保留原始 poisoned
  结果，成功时采用新结果，同时在所有 terminal 分支携带 `retired_session_id`，使 server 的后续
  resume 查询永久排除旧 session。
- Go 还在 completed output、provider error、Codex semantic/no-progress timeout 与 Codex resume
  transport overflow 上应用 `poisoned.go` 分类，确保“表面成功的 fallback 文本”和不可恢复错误
  不会作为成功或可恢复 session 落库。
- Rust 已完整移植 `poisoned.rs` 分类器，provider result 也带 `resume_rejected`、usage 和 session
  metadata，但 production `ProviderAdapter::run_task_inner` 没有调用这些分类器，也没有 fresh retry；
  当前仅把显式 `resume_rejected` 标成 `resume_rejected`，会丢失 Go 的当次恢复、usage 合并和旧
  session 永久退休契约。

本切片必须在既有 provider adapter、execution plan、prompt/environment 和 agent result 类型上完成
同一条生产链：安全的一次 fresh retry、cold context 重建、usage 合并、poisoned output/error/
timeout/transport 分类，以及 every-terminal-path retirement。不得新增第二个 backend factory、task
runner、session registry 或测试专用 production seam。

- 退出证据：真实 production adapter 从 claimed task + accepted launch 进入 provider execution；
  tool-used 路径不 replay 但仍退休 poisoned session；fresh retry 的成功、无新 session 失败、启动
  失败均保持 Go 的 authoritative-result 和 usage 规则；completed fallback 变成失败；普通网络、限流、
  auth、配置和 timeout 不被误判为可 fresh retry；terminal delivery 收到准确 failure reason 与
  `retired_session_id`。
- 默认生产路径：上游 Rust daemon 已默认启动 `DaemonProductionServices -> ProviderAdapter`；本切片
  只修改该真实 adapter 和已有共享类型，不引入 Stub、Noop 或 Fake 的有效生产选择。
- Go 是否可下线：本 poisoned-session 能力收口后不再依赖 Go daemon；AUDIT-005 其余 task/repo/
  reconcile 生命周期、异步结果和最终 AUDIT-001..010 门仍需完成。
- Ready PR #558（`codex/cord-222-poisoned-session-lifecycle-rust`，gap commit `ceaa62ae`，
  implementation commit `232a4afe`）已把当次 fresh retry、tool replay gate、跨 attempt transcript
  sequence/session pin、usage merge、authoritative result、全部 terminal classification 和旧 session
  retirement 作为同一条 production task execution 能力交付。实现只修改既有 provider adapter 与
  已移植 classifier；没有新 runner、factory、registry、依赖或兼容分流。
- 默认生产路径：`TaskExecutionOrchestrator -> DaemonProductionServices -> ProductionProviderAdapter`
  保持唯一真实路径；accepted launch 构造的同一 real backend 最多执行一次 cold retry。有效生产配置
  不会选择 Stub、Noop 或 Fake。
- Go 是否可下线：本 poisoned-session 能力在异步证据收口后不再需要 Go daemon；Go compatibility
  source 和全仓 AUDIT-001..010 退休门仍未完成，当前不能删除全部 Go。
- 异步状态：verification/reviewer 待派发，fixer 尚无本 PR finding。主 agent 仅实际运行并通过
  `git diff --check 2833e418..232a4afe`；未把 compile、tests、rustfmt、locked/offline 或 production
  smoke 记为通过。PR 明确堆叠在 Ready #557，继承 #556 lock 与 #557 format/lock finding。

## 46. [~] AUDIT-005 执行缺口：Codex session rollout durability and continuity

当前切片继续 `AUDIT-005` 的 task execution / crash recovery / terminal delivery 完整生产链：

- Go 在 Codex status 首次暴露 session id 后不会立即写 server；它在 task-owned waiter 中轮询
  per-task `CODEX_HOME/sessions`，仅在 rollout 确实落盘后 mid-flight pin session/workdir。这样 daemon
  crash recovery 不会保存一个下一次必然无法 resume 的指针；非 Codex provider 仍立即 pin。
- Go 在每个 provider terminal result 上再执行一次两秒 bounded rollout flush wait。若 Codex session
  仍不存在，就清空 terminal `session_id` 并设置 `session_rollout_missing=true`；complete/fail/cancel、
  fresh-session retry、usage、branch/workdir 与 failure reason 均继续交付。server 依该布尔值清理已有
  resume pointer，并让下一次 claim 明确注入 continuity-loss 提示。
- Rust 已有 `execenv::codex_home::codex_resume_rollout_present`、`TaskResult.session_rollout_missing`、
  Client complete/fail payload、handler/service/DB 清理链与 claim response 字段，但 production
  `drain_session` 当前对所有 provider 立即 pin，`ProviderAdapter` 也从不检查 rollout 或设置该字段。
  继续追踪 every-terminal 路径后还确认 server-side cancellation 改走 `cancel-ack`，该 payload/handler
  当前同样不携带 missing 信号；若不在 cancelled CAS 下清 task 和 exact-match chat pointer，已 pin 的
  不可恢复 session 仍会越过 terminal gate。两处必须作为同一 continuity 契约收口。

本切片必须复用现有 rollout finder、task-owned cancellation、transcript drain、terminal result 和
Client/service wire：Codex mid-flight pin 等待 rollout 且不阻塞 transcript；waiter 不越过 task owner；
terminal gate 有界等待并在最后时刻复查；complete/fail/cancel-ack 均保留 missing 信号并在同一
cancelled transaction 中 exact-match 清 chat pointer；非 Codex、空 session、正常 rollout 不改变现有
行为。不得新增 session store、watcher service、filesystem index、后台 supervisor 或
provider-specific runner。

- 退出证据：真实 production adapter 对 delayed rollout 会在出现后 pin；missing rollout 在运行结束
  前从不 pin，terminal session id 被 withheld 且所有 success/failure callback 均携带 missing flag；
  immediate rollout 零额外等待；非 Codex 仍立即 pin；取消/terminal 后没有 detached waiter。
- 默认生产路径：Rust `TaskExecutionOrchestrator -> DaemonProductionServices -> ProductionProviderAdapter`
  与既有 `Client` terminal API 保持唯一链路，不引入 Stub、Noop 或 Fake 生产选择。
- Go 是否可下线：本 Codex continuity 能力收口后不再需要 Go daemon；其余 AUDIT-005 生命周期、
  异步收口和最终 AUDIT-001..010 门仍未完成。
交付状态（Ready PR #559）：

- branch `codex/cord-223-codex-session-durability-rust`，堆叠 base
  `codex/cord-222-poisoned-session-lifecycle-rust`（Ready #558）；缺口登记 `aedccda6`，terminal 扩展登记
  `c07b1216`，迁移实现 `dacfe79c`。
- Rust production adapter 现在复用既有 rollout finder：status-time pin 由 task-owned `JoinSet` waiter
  持有，delayed rollout 出现后才调用真实 `Client::pin_task_session`，terminal/drop 均不会遗留 detached
  task；最终 authoritative result（含 fresh retry reconciliation）再执行两秒 bounded gate，missing 时
  withholding session id 并设置 `TaskResult.session_rollout_missing`。
- daemon cancel-ack、Rust handler、TaskService 和 cordy-db cancelled CAS 已接入 missing flag；该事务按既有
  chat-session-first 锁序 exact-match 清理仍指向 cancelled task session 的 chat pointer。没有新增 runner、
  watcher、store、factory、registry、dependency 或 production Fake。
- 当前实现代码 diff 为 6 个必要生产文件、470 insertions/51 deletions；测试直接覆盖 real Client HTTP
  pin/cancel-ack boundary、delayed/missing/immediate rollout 和 non-Codex 路径，而非把几个纯 helper
  测试拼成 production 声明。
- 已知异步 fixer 项：既有 complete/fail service path 会在 task row 记录 missing 并清空 task session，
  但对已经 mid-flight pin 的 chat pointer 尚缺直接 exact-match 清理证据；在 fixer 收口前不声称该分支
  已完成 every-terminal chat-pointer cleanup。
- verification（exact HEAD `f36e24c2`）：`git diff --check` 与
  `git diff --check 08a1f9ee...HEAD` 均通过；fixed-stable rustfmt check 失败，除继承 #558 外含 #559
  `provider_adapter.rs`/`task_execution.rs` 布局差异。daemon 及 db/service/handler 的 locked/offline
  no-run 均在编译前被继承的 #556/#557 Cargo.lock 缺 `procfs`/`procfs-core`/chrono 变更阻断（exit 101）；
  两个 exact 新测试同样未到 discovery，实际 0+0，不能记录通过。verifier 静态确认唯一 production
  chain 已接入，worktree/Cargo.lock 前后 clean/unchanged；runtime smoke 未执行。
- reviewer（exact code HEAD `f36e24c2`）报告：P1 complete/fail missing 只清 task row、未 exact-match
  清已 pin chat pointer；P1 server cancellation 会让 drain 合成空-session result，fresh retry cancel 因而
  不产生 missing flag，且 cancel-ack 仍需承接 #558 的 `retired_session_id`；P2 现有 drain/helper/JSON
  测试尚未直接覆盖 production retry、terminal callback 与 DB transaction。Ponytail review 认为六个
  production 文件均属必要 wire/transaction 链，问题是缺失行为/证据而非过度抽象。
- fixer 已排队合并上述 review、known gap、格式及 inherited lock/test 阻断；要求在既有锁序事务中
  exact-match 清 current/retired pointer，并补 complete/fail/cancel、newer-pointer 与 fresh-retry cancel
  present/missing 证据。主 agent 未运行或伪报 compile/tests/rustfmt/production smoke；PR 继续继承 #558
  findings 及更早 #556/#557 lock/format 基线记录并保持 Ready。

## 47. [~] AUDIT-003A 执行缺口：Rust allocation heap profile and async runtime diagnostics

当前切片回到尚未退出的 `AUDIT-003A`，迁移 Go profiling 的剩余真实运维能力，而不是继续把进程
总量指标或 410 retirement response 当成 heap/runtime diagnosis：

- Go `net/http/pprof` 在默认 loopback listener 上按需导出 allocator allocation-stack heap pprof，
  并按 caller duration 采集 goroutine/scheduler/runtime events；Rust 当前 `/debug/pprof/heap` 与
  `/debug/pprof/trace` 均只返回 410，PR #556 的 RSS/virtual-memory/thread/fd metrics 不能回答
  “哪条分配栈持有内存”或“哪个 async task/resource/operation 阻塞调度”。
- Linux Rust server 必须使用一个真实 profiling allocator，并让现有
  `127.0.0.1:6060/debug/pprof/heap` 导出可由 pprof 读取的 gzipped protobuf allocation profile；
  capture 失败、profiler 未激活或临时目录不可写必须返回非 2xx，不能返回空或伪造 profile。
- Rust 没有 Go runtime trace wire contract。本切片以 Tokio 官方 task/resource/operation telemetry
  作为语义替代，并固定绑定第二个 loopback-only management address；它必须合并到现有 production
  tracing subscriber，保留 LOG_LEVEL、时间、ANSI 和 request/component logs，不能用普通结构化日志
  或 CPU profile 冒充 scheduler diagnostics。
- 非 Linux release 继续编译运行，但 heap endpoint 明确 fail-closed 并说明 Linux-only contract；
  async diagnostics 不得因环境变量覆盖而绑定公网。Docker/release build 必须携带 Tokio runtime
  instrumentation 所需 workspace Cargo config，不能只在开发机偶然生效。
- 复用现有 profiling router、server startup 和 tracing subscriber；只引入 jemalloc pprof converter、
  allocator 与 Tokio console layer，不新增 profiler service abstraction、factory、registry、config parser、
  fake trace encoder 或第三套日志初始化。

退出证据必须直接覆盖 production binary，而不是几个 helper：Linux locked release build 使用 profiling
allocator；启动后 heap route 返回非空 gzip pprof 且 public API 不暴露该 route；Tokio console client 能从
固定 loopback endpoint 观察 server 实际 task/resource/operation；非法 capture 环境和非 Linux heap
fail-closed；SIGTERM/shutdown 与既有日志输出不回归。compile、format、tests、真实 capture、console client、
Docker build 和开销观察全部交独立 verifier，finding 交独立 fixer；主 agent 不代跑或代修。

交付状态（Ready PR #560）：

- branch `codex/cord-224-rust-heap-trace-contract`，堆叠 base
  `codex/cord-223-codex-session-durability-rust`（Ready #559）；缺口登记 commit `ce6bbd54`，production
  implementation commit `c6ee2f10`。
- Linux `cordy-server` 生产 binary 使用带 profiling 的 jemalloc；现有固定 loopback profiling router 的
  heap route 从真实 allocator dump 返回 non-empty gzipped pprof，inactive/empty/dump failure 均非 2xx；
  非 Linux 明确返回 501，不伪造 profile。
- 现有 production tracing subscriber 同时安装 Tokio console layer，固定绑定 `127.0.0.1:6669`，保留
  LOG_LEVEL、本地毫秒时间、ANSI 与 structured fields；legacy Go trace route 诚实返回 410 并指向真实
  task/resource/operation endpoint。Docker 带上 workspace `tokio_unstable` instrumentation config，运维
  文档覆盖 bare-metal、container heap capture 与 network-namespace console client。
- 实现只触及 allocator/subscriber、既有 profiling router、crate manifest、Docker build 和现有 runbook
  六个必要文件，共 162 insertions/49 deletions；无新 service abstraction、factory、registry、config
  parser、Fake 或第二套 logger。
- 异步状态：independent verifier/reviewer 待派发，finding 交 independent fixer。主 agent 仅实际运行并
  通过 `git diff --check 0a4c4258..c6ee2f10`；Cargo.lock/resolution、rustfmt、compile、tests、Linux release
  pprof decode、Tokio console client、public isolation、shutdown、Docker/non-Linux build 与 overhead 尚未
  记为通过。PR 保持非 Draft Ready，不等待异步收口。
- verification（exact HEAD `47897fca`）：两个 diff check 和直接触及的 main/profiling fixed-stable
  rustfmt 通过；Cargo.lock 前后 SHA256 均为
  `a40395d7b03895b86d2e8fdc717d492edf28ad94f667f7ed7555271880cb2dc4` 且 worktree clean。但 lock
  不含新增 `console-subscriber`/jemalloc graph，full `cargo metadata --locked --offline` 在解析阶段以
  `no matching package named console-subscriber found` exit 101；server no-run、heap exact test 和 Windows
  check 同样在 discovery/compile 前 exit 101，heap exact 实际 0 tests，不能记为通过。Linux release、
  live heap gzip/pprof decode、public isolation、Tokio console client、shutdown、Docker build 与 overhead
  全部未执行。静态检查只能确认 allocator/route/fixed-loopback wiring 存在，不能替代 runtime evidence；
  完整 lock 及后续暴露问题已排给 independent fixer，修后必须重跑上述验证。
- reviewer（exact code HEAD `47897fca`）报告两个 P1、三个 P2、一个 P3：除缺 lock 外，当前
  `ConsoleLayer::spawn` 自建 detached thread/runtime，6669 bind failure 只 panic 该线程，未进入 startup
  failure、root cancellation 或 graceful join；heap tempfile/dump/parse/gzip 直接阻塞 Tokio worker；全
  workspace `tokio_unstable`、server tracing 与 console 默认一小时 retention 无条件常驻，尚无安全默认、
  容量上界或负载开销证据；helper tests 未直接覆盖 production binary/pprof decode/console client/public
  404/conflict/shutdown/musl/non-Linux；runbook 的 `sudo nsenter ... tokio-console` 通常找不到用户
  `~/.cargo/bin`。Ponytail 未发现多余 factory/registry/file；问题是 lifecycle、blocking boundary、默认
  成本和直接证据。全部 finding 已交 independent fixer，当前不声明该切片已完成 AUDIT-003A 退出。

## 48. [~] AUDIT-005 执行缺口：confirmed provider demotion and recovery lifecycle

当前切片选择 `AUDIT-005` registration/reconcile 的完整 machine-provider 生命周期，而不是只接一个
demotion helper：

- Go `detectBuiltinRuntimes` 将 built-in probe 分成三类：可用、暂时无法读取版本、已确认低于最低
  版本或 OS 拒绝执行。暂时失败必须保留已注册 runtime；只有 confirmed verdict 才能下线。not-
  executable 必须跨 confirmation window 再现，并携带稳定 `not_executable` code、detail 与可识别 npm
  package 的 postinstall repair command。
- Go `demoteUnusableRuntimes` 在 claim barrier 下原子移除所有 workspace 的 condemned built-in
  runtime 与 launch/version state，再按 workspace registration lock 重检仍未跟踪的 ID，向真实
  `/api/daemon/deregister` 发送 per-runtime offline reasons；custom-profile runtime 永不被该路径移除。
- Go 以 seq-stamped provider hold 拒绝 demotion 前已发出、demotion 后才返回的 register response；
  被拒绝的 revived row 必须再次带原原因下线。后续 probe 确认恢复时，只能清除不晚于该 probe
  sample 的 hold，避免旧 probe 覆盖较新的 demotion；恢复后由既有 converge/register 路径重新上线。
- Rust 已有真实 `LocalProviderCatalog -> ProviderRegistrationSource -> RuntimeRegistrationService ->
  RuntimeRegistry/RuntimeLaunchRegistry -> Client` 生产链，也已有 claim barrier、workspace serial、
  `RuntimeOfflineReason` wire type 和 `agents_refresh::partition_demotable_runtimes`，但 production catalog
  只把全部失败静默省略，registration 又把省略一律当 authoritative removal。现有 demotion
  partition/`RevivedRuntimes` 仅在孤立模块测试中使用，生产调用方为零；offline reasons 永远以空 map
  发送，且没有 late-response hold 或 recovery ordering。

本切片必须复用上述真实 production objects，把 probe verdict、transient preservation、confirmed
demotion、claim/workspace ordering、structured deregister、late response rejection 与 generation-safe
recovery 接成同一闭环。不得新增第二个 catalog、registry、supervisor、background loop、client 或测试
专用 production seam；custom-profile、runtime-gone、profile refresh、version refresh 与 startup shared
probe 必须继续走既有 registration service。

- 退出证据：暂时 probe failure 不删除 runtime/launch；below-minimum 和跨窗口 not-executable 在无
  active claim/task 后从 authoritative runtime set 消失并向 server 发送准确原因；custom profile 保留；
  demotion 前 in-flight response 不能复活 provider，若 server 已 upsert 则被再次下线；新 probe 不可
  清较新 hold；真实恢复 probe 能清旧 hold并重新注册；deregister 前 recheck 不会击落更新的 row。
- 默认生产路径：Rust CLI 构造的唯一 `LocalProviderCatalog` 与 `DaemonProductionServices` 直接使用该
  契约；缺失/非法/不支持 provider 继续 fail-closed，不选择 Stub、Noop 或 Fake。
- Go 是否可下线：本 provider demotion/recovery 能力在异步证据收口后不再需要 Go daemon；
  AUDIT-005 其余生命周期、全仓 Go compatibility source 和最终 AUDIT-001..010 门仍未完成。
- Ready PR #561（`codex/cord-225-daemon-registration-reconcile-rust`，堆叠 base
  `codex/cord-224-rust-heap-trace-contract` #560，gap commit `39c332e0`，implementation commit
  `a1534d40`）已把 probe verdict、transient preservation、claim barrier、runtime/launch removal、
  structured offline reason、late-response hold、revived-row cleanup、generation-safe recovery 与
  deregister recheck 作为一个完整 production lifecycle 交付。
- 实现只修改五个既有 daemon 文件，共 855 insertions/70 deletions（含直接 registry/probe contract
  tests）；没有新增 crate、dependency、catalog、registry、client、supervisor、background loop、
  Stub、Noop、Fake 或测试专用 production seam。
- 异步 verification（exact head `21a3ab795bd9f1e5c53ea54a712cda1486a43b42`）：工作树前后均
  clean；`git diff --check` 与 `git diff --check 9c4227c4...HEAD` 通过；`Cargo.lock` 前后 SHA-256
  均为 `a40395d7b03895b86d2e8fdc717d492edf28ad94f667f7ed7555271880cb2dc4`。固定 stable
  `rustfmt --edition 2021 --check` 失败，`provider_registration.rs`、`registration.rs`、
  `runtime_registry.rs` 有 import/order/wrapping diff。`cargo metadata --locked --offline` 与
  `cargo test --locked --offline -p cordy-daemon --lib --no-run` 均以 101 退出：继承自 #560 的
  dependency-lock inconsistency 导致 `--locked` 拒绝更新 lockfile，尚未进入编译。五个新增精确测试
  均以同一 101 在 discovery/execution 前退出，实际各执行 0 个测试；loopback 与真实 production smoke
  未运行。因此所有行为仍是 execution-unverified，未记为通过。
- 异步 reviewer（只读 exact product head `21a3ab795bd9f1e5c53ea54a712cda1486a43b42`）无 P0，
  报告 4 个 P1：workspace serial→claim barrier 与 demotion barrier→serial 锁序反转可永久互等；fresh
  resolver miss 被误当 authoritative disappearance；runtime identity 与 launch spec 分两次发布存在 claim
  中间窗口；provider probe 串行化使最坏启动/refresh 延迟退化为全部 CLI timeout 之和。另有 4 个 P2：
  busy daemon 应 defer demotion 而非冻结所有新 claim；单 workspace deregister 失败不应中止其余且必须可
  重试；真实 health 未暴露 `skipped_agents`；无 minimum floor 的 CLI 空输出未保留 last-known version。
  P3 记录当前 locked compile/测试 0-run/rustfmt failure 不能支持 production lifecycle 已验证措辞，并要求
  锁序、registry/launch interleave、真实 Client deregister continuation 的直接检查。未发现 Stub/Noop/Fake
  误入 production，五个既有文件与对象的复用方向符合 Ponytail。
- 上述 lock/rustfmt/compile/test/smoke 和 reviewer findings 已排入 existing independent fixer，待其完成
  #560 后处理。主 agent 仅迁移、接线、交付并回写事实，不自行修复；PR 保持非 Draft Ready，不等待异步
  收口。

## 49. [~] AUDIT-005 执行缺口：private socket-safe task temp lifecycle

当前切片继续 `AUDIT-005` 的 task execution / provider launch 完整生产链，而不是只替换一个环境变量：

- Go `ensureTaskTempDir` 在每次 task run 中、provider 启动前创建独立的短路径临时目录，POSIX 权限为
  `0700`，并把同一路径覆盖到 child 的 `TMPDIR`/`TMP`/`TEMP`；目录在成功、失败、取消和 provider
  launch error 后都清理。默认 Unix base 优先 `/tmp` 以给 agent 的 AF_UNIX socket 留出 path headroom；
  Windows 使用平台 temp。
- 非 Windows production operator 可通过 `CORDY_AGENT_TEMP_BASE` 把目录迁移到指定 absolute base；相对
  路径或真实创建失败必须 fail-closed，不能静默回退。Windows 明确忽略该 override。agent `custom_env`
  不能覆盖 `CORDY_*` 或 `TMPDIR`/`TMP`/`TEMP`，所以 daemon-owned private path 始终权威。
- 当前 Rust `ProductionProviderAdapter::run_task_inner` 直接使用
  `predict_root_dir(...)/tmp` 并 `create_dir_all`。长 workspaces root/task ID 会把 child socket 路径推过
  平台上限；路径不是 per-run 随机目录，也没有 `CORDY_AGENT_TEMP_BASE` contract。现有 execution plan 已
  集中覆盖三个 temp env key，production adapter 也已有统一退出清理点；应复用这些入口，不新增 temp
  manager、trait、crate 或 dependency。
- 当前仓库历史中已有未进入本迁移堆栈的 task-temp 实现提交
  `dcdba747`/`f38de802`/`90e844b3`；Ponytail 要求复用其已验证设计，但必须按当前 production adapter/
  execution-plan 结构做最小移植，不能机械 cherry-pick 旧分支的无关 merge 或已被当前堆栈替代的代码。

- 退出证据：真实 production task 在长 env root 下获得短路径、存在且 private 的独立 temp dir；三个
  child env key 一致且不可被 agent env 覆盖；unset/valid/relative/unwritable override 行为符合平台契约；
  两次同 task run 不复用目录；completed/failed/cancelled/launch-error 后均无残留。
- 默认生产路径：Rust CLI daemon 唯一 `ProductionProviderAdapter` 在 `StartTask` 和 provider launch 前
  创建并注入该目录；缺失或非法 operator 配置 fail-closed，不选择 Stub、Noop 或 Fake。
- Go 是否可下线：本 private task temp 能力完成并经异步证据收口后不再需要 Go daemon；AUDIT-005
  其余 daemon lifecycle、全仓 Go compatibility source 与最终 AUDIT-001..010 门仍未完成。
- Ready PR #562（`codex/cord-226-private-task-temp-rust`，堆叠 base #561 branch
  `codex/cord-225-daemon-registration-reconcile-rust` at `93d38c16`；gap commit `b4a6811a`，production
  implementation `e4fc2b7d`）已把 per-run socket-safe allocation、POSIX `0700`、absolute operator
  override/fail-closed、三个 child temp env key、prepare lease ordering 与所有退出路径 cleanup 接入唯一
  `ProductionProviderAdapter`。
- 实现只修改三个既有 daemon 文件，共 260 insertions/13 deletions（含 allocation/env 直接 contract
  checks）；没有新增文件、crate、dependency、temp manager、trait、adapter、Stub、Noop 或 Fake。实现复用
  当前仓库已有 `tempfile`/execenv/execution plan/prepare lease，并按当前堆栈移植历史设计而非 cherry-pick
  无关 merge。
- independent verifier 在 exact HEAD `d0fd2a073e3559e2145591ff9c2d10d24db7d02a` 确认 branch/base range
  与前后 clean worktree；`git diff --check`、`git diff --check 93d38c16...HEAD` 和 Cargo.lock unchanged
  check 均通过，lock SHA-256 前后都是
  `a40395d7b03895b86d2e8fdc717d492edf28ad94f667f7ed7555271880cb2dc4`。fixed stable rustfmt 在
  `provider_adapter.rs` 失败；另两个改动文件无 diff。继承自 #560 的 dependency-lock inconsistency 使
  `cargo metadata --locked --offline`、daemon lib no-run、五个新增 exact tests 和既有 blocked-custom-env
  exact test 全部在 compile/discovery 前 exit 101，每项实际执行 0 tests，不能记录为通过。production
  adapter、long-path、override、permission、completed/failed/cancelled/start/launch-error cleanup、prepare
  lease 与 Windows runtime smoke 均未执行；verifier 只静态确认 allocation/env/ordering/cleanup wiring，
  不作为运行通过证据。
- 上述 inherited lock、#562 rustfmt、compile/test 与 runtime smoke 已排给 existing independent fixer；
  主 agent 不自行修复或复验，PR 保持非 Draft Ready，不等待异步收口。
- independent reviewer 在同一 exact HEAD 无 P0/P1，报告 3 个 P2：Rust 未 trim
  `CORDY_AGENT_TEMP_BASE`，使 whitespace-only 或带首尾空格的 absolute value 与 Go 语义不等价；helper
  接受 non-Unicode absolute override，但唯一 production adapter 随后必然在 `to_str` 处拒绝，现有
  “accepts” test 与生产声明矛盾且错误未点名 operator variable；新增检查没有穿过
  `ProductionProviderAdapter::run_task_inner`，不能直接证明 StartTask/child env/prepare lease 与
  success/failure/cancel/start/launch-error cleanup。另有 1 个 P3/Ponytail：同一 test module 的三份
  `EnvRestore` 完全重复，应合并为一个 test-only helper。其余三文件复用、唯一 production adapter、
  custom-env gate、0700 创建和 guard lifetime 核对无 finding。全部 finding 已排给同一 independent
  fixer，尚无 fix SHA 或重新验证结果。

## 50. [~] AUDIT-005 执行缺口：wakeup environment proxy and CONNECT lifecycle

当前切片继续 `AUDIT-005` 已迁移的 wakeup/WS RPC/control 生产链，补齐企业网络中真实连接能否建立的
完整边界，而不是新增一个孤立 proxy parser：

- Go `runTaskWakeupConnection` 的手工 `websocket.Dialer` 明确使用 `http.ProxyFromEnvironment`；wss
  target 按 `HTTPS_PROXY` 选择 HTTP CONNECT proxy，`NO_PROXY` 可绕过，未配置时保持 direct。proxy URL
  credentials 由 CONNECT `Proxy-Authorization` 使用；`wakeup_proxy_test.go` 直接以真实
  `wakeup.example.invalid:443` target 证明生产 dial 发出 CONNECT。否则 corporate-egress daemon 永远无法
  建立 control socket，只会静默退化为 HTTP polling。
- Rust 唯一 production `DaemonManager::run_task_wakeup_connection` 已构造真实 authenticated request、
  message limits、handshake timeout 和完整 control owner，但直接调用
  `tokio_tungstenite::connect_async_with_config`，仓库中没有任何 `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY`
  选择或 CONNECT。现有 #549 loopback 检查不设置代理，删除或遗漏该能力仍会通过。
- 当前 workspace 已有 `url`、`base64`、`tokio`、`tokio-tungstenite`，且 Lark production connector 已有
  bounded HTTP CONNECT、Basic auth、target TLS-after-tunnel 的既有设计。实现必须复用这些现有依赖与
  handshake API；不新增 proxy crate、manager、trait、client、background loop 或第二条 wakeup path。

本切片必须在唯一 manager dial 中接通 Go-compatible environment selection、`NO_PROXY` bypass、bounded
HTTP CONNECT response、optional Basic proxy auth、target TLS/WebSocket handshake、既有 bearer/capability
headers、timeout/cancellation 与 polling fallback。非法/unsupported proxy 配置必须返回明确 transport
error 并继续既有 bounded retry/fallback，不能 silently direct 绕过 operator policy；proxy credentials
不得进入日志或 target WebSocket headers。

- 退出证据：真实 production dial 对 wss + HTTPS_PROXY 向 stand-in proxy 发出 exact target CONNECT，
  200 后继续 target TLS/WS handshake；credential proxy 只收到正确 Basic auth；NO_PROXY/unset 走 direct；
  malformed scheme/host、非 2xx、oversized/truncated response fail-closed；既有 auth/capability headers、
  message limits、RPC/control consumer、cancellation 和 HTTP polling fallback 不回归。
- 默认生产路径：Rust CLI daemon 构造的唯一 `DaemonManager` 直接使用该连接边界；没有 Stub、Noop、Fake
  或 test-only production selector。Go 是否可下线：本 proxy 能力经异步证据收口后不再需要 Go daemon；
  AUDIT-005 其余生命周期和最终 AUDIT-001..010 门仍未完成。
- implementation commit `544a603a` 已在唯一 `DaemonControl::run_task_wakeup_connection` 接入
  environment selection、NO_PROXY/loopback bypass、HTTP-only proxy fail-closed、Basic auth、16 KiB bounded
  CONNECT 和 tunnel 后 target TLS/WS handshake。真实 production child-process check 直接以 wss target、
  `HTTPS_PROXY` credentials 与 stand-in proxy 断言 exact CONNECT、auth 不泄到 target request、200 后发出
  TLS ClientHello；另有 selection/NO_PROXY/malformed/unsupported 与 refusal/truncation/oversize checks。
- 实现只修改现有 `manager.rs` 和 daemon manifest，共 377 insertions/1 deletion；复用 workspace 已安装并
  已锁定的 `hyper-util` matcher（只为 daemon 启用现有 `client-proxy` feature）、`tokio` 和
  `tokio-tungstenite`，未新增 crate、proxy manager/trait/client/background loop、Stub、Noop 或 Fake。
- 默认 production Rust CLI daemon 仍只构造同一 `DaemonControl`；unset/NO_PROXY/loopback 保持 direct，
  有效 environment proxy 走 CONNECT，malformed/non-UTF-8/unsupported scheme、connect/write/read/status 错误
  进入既有 transport retry 与 HTTP polling fallback，不 silently direct。Go 本能力待异步证据收口后可退休；
  全局 Go 删除仍依赖 AUDIT-001..010。
- owner：主 agent 仅迁移、接线与 Ready PR 交付；独立 verifier 负责 compile/test/real CONNECT smoke，
  reviewer 只读审查，finding 交 existing independent fixer。依赖 #562 branch
  `codex/cord-226-private-task-temp-rust` at `1bf5cccd`。主 agent 只运行并通过 `git diff --check`；Cargo
  resolution、Cargo.lock、rustfmt、compile、tests、real CONNECT/TLS/control/fallback 与平台检查均尚未由
  verifier 执行或记为通过。
- Ready PR #563：branch `codex/cord-227-daemon-wakeup-env-proxy-rust`，gap commit `2c0d0577`，
  implementation `544a603a`，initial delivery ledger `9cafc275`；base 是 Ready #562 branch
  `codex/cord-226-private-task-temp-rust` at `1bf5cccd`。PR 为非 Draft Ready。
- independent verifier 在 exact HEAD `2180ded88a00f48fe31e60e09d963c6d1c936a6a` 确认 worktree clean、
  range/diff check 通过且 Cargo.lock SHA 未变；fixed stable rustfmt 在 `manager.rs` 失败。daemon 对
  workspace `hyper-util` 启用 `client-proxy` 时同时继承无效 `runtime` feature，故 metadata、daemon
  no-run、三个新增 exact tests、既有 real websocket exact test 和 Windows check 全部在编译/测试发现前
  exit 101、各执行 0 tests；继承自 #560 的 lock blocker 尚未到达。production CONNECT/TLS/control smoke
  因此没有运行，不能记录为通过。
- independent reviewer 在同一 exact HEAD 无 P0，报告 3 个 P1：environment lower/uppercase precedence 与
  CGI 拒绝不等价；`hyper-util` host-only matcher 缺失 Go `NO_PROXY` 的 port/IPv6/leading-dot/`*.` grammar
  且测试锁定错误 apex 语义；IPv6 proxy host 的方括号会破坏 dial。另有 P2：handshake 阶段 root cancel
  不生效、CONNECT status parser 过宽、proxy userinfo auth parity 缺口及证据/退休声明过宽；P3 指出现有
  Lark 已有近似 CONNECT 实现而 daemon 重复协议逻辑。全部 verifier/reviewer findings 已排入 existing
  independent fixer；主 agent 不修复、不等待，PR 保持 Ready。

## 51. [~] AUDIT-005 执行缺口：heartbeat stale HTTP pool recovery lifecycle

当前切片继续 `AUDIT-005` 的 control/heartbeat 生产生命周期，迁移 server restart、NAT/LB stale keepalive
后的真实恢复能力，而不是只保留一个同名 no-op：

- Go `runRuntimeHeartbeat` 将 404/runtime-gone 与 transient transport/5xx 分开；连续两次 transient failure
  后调用真实 `http.Transport.CloseIdleConnections()`，使下一 heartbeat 不再复用 stale pooled socket。成功、
  runtime-gone 或 WS heartbeat freshness 会重置 failure streak；in-flight request 不被取消。Go
  `TestRuntimeHeartbeatClosesIdleConnectionsAfterRepeatedTransientFailures` 直接固定该调用阈值。
- Rust `DaemonControl::run_runtime_heartbeat` 已保留相同 failure counter、第二次 failure 调用点、404/event
  和 success reset；但 `Client::close_idle_connections` 明确是 best-effort no-op，只依赖 90s idle timeout。
  因此源码表面有 parity，真实 client pool 没有任何 eviction，server restart 后可能连续复用失效路径并把
  恢复延迟到 pool timeout。
- reqwest 0.12 没有公开 close-idle handle，但 `reqwest::Client` clone 共享同一 pool；最小真实边界是在现有
  `Client` 内原子替换用同一 builder 构造的新 client。已开始的 request 持有旧 clone 继续完成，旧 pool 在
  最后一个 in-flight clone drop 后关闭；后续所有 centralized request builders 取新 clone。不得新增第二个
  API client type、transport trait、heartbeat supervisor、generation registry 或 dependency。

本切片必须把 HTTP client construction 收敛为现有 `Client` 内一个 helper，所有 GET/POST/token paths 从同一
可替换 handle 建 builder，并让现有连续 failure 调用点真实换池。替换必须保留 environment proxy、TLS、
identity/auth headers、per-request timeout、retry 和 cancellation；只清连接池，不清 token、ETag/workspace
cache、legacy endpoint state 或 runtime registry。

- 退出证据：同一 production `Client` 在 eviction 前复用 keepalive，eviction 后下一请求建立新 TCP
  connection；正在执行的请求不因 swap 被取消；production heartbeat 仅在第二个连续 transient failure 后
  触发 swap，success/404/WS freshness 重置 streak；后续 heartbeat 可恢复且 auth/identity headers 不回归。
- 默认生产路径：Rust CLI daemon assembly 只构造这一 `Client` 并注入唯一 `DaemonControl`；没有 Stub、
  Noop、Fake 或 test-only production selector。Go 是否可下线：本 stale-pool recovery 经异步证据收口后
  不再需要 Go daemon；AUDIT-005 其余生命周期和最终 AUDIT-001..010 门仍未完成。
- owner：主 agent 迁移、接线与 Ready PR；独立 verifier/reviewer/fixer 异步收口。依赖 Ready #563 branch
  `codex/cord-227-daemon-wakeup-env-proxy-rust` at `2180ded8`。
- implementation commit `762be14d` 将现有 `Client` 的唯一 reqwest handle 改为原子可替换 pool handle，
  construction 仍收敛在同一个 builder；所有 centralized workspace GET、authenticated/explicit-token GET、
  POST builder 都在建 request 时克隆当前 handle。换池前先构造 replacement，锁只覆盖 handle swap；已经建成
  的 request 继续持有旧 pool，token/version/workspace ETag/cache/legacy endpoint 和 runtime state 均不变。
- 直接 client contract check 在第一个 TCP connection 上证明 keepalive 复用，并在第二个请求 in-flight 时
  换池：该请求成功完成，而下一 request 必须由第二个 TCP connection 接收。直接 production heartbeat
  check 让唯一 `DaemonControl::run_runtime_heartbeat` 经历 `500,500 / 200 / 500,500 / 200`，服务器必须依次
  接受三个 pool connection，因而同时固定第二次连续失败才换池以及成功后 streak reset；原有 404/runtime-
  gone 和 WS-freshness reset 分支未改动。
- 实现只修改现有 `client.rs` 与 `manager.rs`，共 199 insertions/14 deletions（大部分为上述两项真实 socket
  contract checks）；没有新增文件、crate、dependency、client type、transport trait、supervisor、generation
  registry、Stub、Noop 或 Fake。唯一 Rust production assembly 仍构造这一 `Client` 并注入同一
  `DaemonControl`，现有第二次 failure 调用点现在执行真实 pool retirement。
- 主 agent 只运行并通过 `git diff --check`；rustfmt、Cargo resolution/lock、compile、exact tests、完整 daemon
  tests、production runtime smoke 与平台检查尚未由 independent verifier 执行，不能记录为通过。Ready
  PR #564（branch `codex/cord-228-daemon-http-pool-recovery-rust`；gap `616a7d64`、implementation
  `762be14d`、initial delivery ledger `5710efde`）堆叠在 Ready #563 branch at `2180ded8`，保持非 Draft；
  Go 本 stale-pool recovery 只有在异步证据收口后可退休，AUDIT-005 其他缺口和最终 AUDIT-001..010
  仍未完成。
- independent verifier 在 exact HEAD `49bfaf87a499015b030979c96aa6be6b538644ec` 确认 base ancestry、
  前后 clean、worktree/range diff check 通过，Cargo.lock 未变且 SHA-256 仍为
  `a40395d7b03895b86d2e8fdc717d492edf28ad94f667f7ed7555271880cb2dc4`。fixed-stable
  rustfmt exit 1：除继承 #563 的 proxy diff 外，本切片 heartbeat test 的 signature 和两个 response call
  也需格式化。metadata、daemon no-run、Windows check、两个新增 exact tests 与四个相关既有 exact tests
  均被继承自 #563 的 `hyper-util` invalid `runtime` feature 在 dependency resolution 阶段以 101 阻断，
  每项实际执行 0 tests；生产 socket/loopback smoke 未运行，只完成静态 production call-chain 核对，不能
  记录行为通过。direct format finding 与 blocker 后的全部 rerun 已排入 independent fixer，主 agent 不修复、
  不等待。
- independent reviewer 在 exact product HEAD `49bfaf87a499015b030979c96aa6be6b538644ec` 无 P0，
  报告 1 个 P1：Rust heartbeat 当前把除 404 外的所有错误都累计 streak，且任意 404 都发 RuntimeGone；Go
  只让 transport/5xx/408/429 累计，永久 4xx/cancellation 重置，并要求 404 body 明确包含
  `runtime not found`。现有 `client::is_transient_error` 已有正确分类但 production heartbeat 未复用，新增
  socket test 只覆盖 500。P2 指出台账对 404/WS reset、auth/identity、state preservation 和退休证据的措辞
  宽于直接测试；须把静态 wiring 与实际运行证据分开。reviewer 同时确认 pool swap lifetime、全部集中 request
  builder、独一 production `Arc<Client>` 和两文件最小抽象方向成立，未发现 Stub/Noop/Fake/alternate path。
  finding 已排入同一 independent fixer，PR 继续 Ready，未把该能力标记为已验证或可删除 Go。

## 52. [~] AUDIT-002 执行缺口：issue-status production API and transaction contract

当前切片选择 `AUDIT-002` 已列出的 API authentication/authorization、transaction、error JSON 与 side-effect
smoke，并以一个完整 workflow contract 迁移 Go issue-status 回归，不按 `issue_status*_test.go` 文件或单个
helper 拆 PR：

- Go `internal/handler/issue_status.go`、`issue_status_test.go`、`issue_status_reorder_test.go` 和
  `issue.go` 的 status guard 共同定义能力：旧 workspace 在 list 时幂等补齐七个系统状态；任意 member 可读，
  只有 owner/admin 且 rollout flag 开启可创建；系统状态不可修改/归档；custom status 继承 canonical category；
  archive 只禁止未来 assignment，不改写已有 issue。
- archive 使用 exclusive catalog transaction lock；所有写入 custom status 的 issue create/update/batch path 在同一
  transaction 内取 shared lock 并重新 resolve。两种先后顺序必须分别表现为“writer 先提交后 archive”或
  “archive 先提交后 writer 409”，不能把 issue 写到已归档状态。reorder 同样在 shared lock 下核对 category 的
  全部 active custom IDs、原子更新全部 position；partial/duplicate/cross-category/cross-workspace/system/archived
  input 必须 fail-closed，竞态失败不得留下部分 position。
- mutation event 只可在 transaction commit 后发布 `issue_status:changed`，payload 只含 action；拒绝、冲突和
  rollback 不得发布。HTTP status 与 `{"error": ...}` wire shape、UUID validation、workspace scoping、archived
  nullable timestamp、canonical order 和 stored custom key 都是本契约的一部分。
- Rust 默认 router 已把 `cordy_handler::issue_status::router` 放在 workspace member middleware 后，handler 直接
  使用 `HandlerState` production pool/bus、`cordy_service::issue_status` 与 `cordy_db::queries::issue_status`；
  issue create/update/batch 已有 status guard 调用点。但 handler 当前只有 color unit check，service 只有局部
  optional-DB checks，不能直接证明真实 handler、transaction lock、error JSON、event ordering 和 issue write
  chain，也不足以退休对应 Go contract。

本切片必须复用现有 handler/service/query/router/state，不增加 repository、service、mock transaction、第二套
router 或 test-only production seam；在既有 Rust 文件内用真实 Postgres fixture 和真实 handler arguments 建立
一组直接 contract checks，覆盖上述 happy path、拒绝路径和两个真实并发 interleaving。fixture 必须按 workspace
隔离并显式清理，不能依赖共享固定 ID；无 `DATABASE_URL` 时必须明确报告未执行，不能伪报通过。

- 默认生产路径：Rust `cordy-server` 只挂载这一 issue-status router 和同一 issue mutation handler/service/query；
  不存在 Stub、Noop、Fake 或 alternate production catalog。有效 production feature source 决定 create gate；
  missing/disabled flag fail-closed，read/resolve 继续兼容 rollout 前 workspace。
- 退出证据：真实 DB 上从 handler request/response 穿过 query/transaction lock/commit/bus，证明 catalog、role/flag、
  immutable/archive、custom assignment、reorder atomicity、archive races、event ordering 与 wire errors；独立
  verifier 负责执行，reviewer 负责比对 Go，失败与 finding 只交 fixer。
- Go 是否可下线：本契约和异步 finding 收口后，Go issue-status API/guard 回归不再是 Rust production 的依赖；
  AUDIT-002 其余 API/WS/background worker、AUDIT-001..009 和最终 AUDIT-010 仍未完成。
- owner：主 agent 只迁移契约、生产入口证据和 Ready PR；独立 verifier/reviewer/fixer 异步收口。branch
  `codex/cord-229-issue-status-production-contract-rust`，依赖 Ready #564 branch
  `codex/cord-228-daemon-http-pool-recovery-rust` at `b68910a4`。

实现 commit `889324d9` 在一个既有 Rust 文件内加入完整 production contract checks，没有新增 dependency、fixture
framework、repository、service 或 test-only production seam：

- 直接调用真实 list/create/update/archive/reorder handler arguments，证明 member read、admin/flag write gate、旧
  workspace 七个 built-in self-heal、系统状态不可归档、custom create/color/category、duplicate error JSON、active/all
  list wire shape 和 stored custom key；
- 用真实 Postgres workspace 隔离 fixture 证明 partial reorder rollback 不改 position/不发 event，exact active set reorder
  原子提交；成功 mutation 的同步 bus action 恰为 created/created/reordered/archived，拒绝和冲突不发布；
- 用真实 `IssueService::create` 和 production issue PUT/batch-update router 穿过 shared catalog lock + in-transaction
  re-resolve；exclusive archive 先提交时 create/update 拒绝，create/update 先提交时 archive 等待、随后只阻止未来
  assignment，已提交 issue 保留 custom key 且 effective category 不变；
- DB fixture 在正常路径显式删除 issue、issue_status 和 workspace；无 `DATABASE_URL` 或连接失败会明确输出 skipped，
  不把 0 个真实 DB case 误报为通过。

主 agent 只运行 `git diff --check`（PASS），没有运行 cargo/rustfmt/测试。非 Draft Ready PR #565 已从 branch
`codex/cord-229-issue-status-production-contract-rust` 创建并明确以 #564 branch 为 base；独立 verifier/reviewer
已异步派发，fixer 尚未接收 finding。因此当前只能标记为 implementation delivered/pending async evidence，不能
声称已验证，也不能删除 Go。

独立 verifier 在精确 HEAD `3e8fff848d9cb9929502fb8309a1058205b28e47` 完成首轮机械验证：worktree/range
`git diff --check`、base ancestry 和 Cargo.lock 未变化均 PASS；fixed stable rustfmt 对本 PR 新增 test code 返回
exit 1。`cargo metadata --locked --offline`、handler no-run、Rust server/Windows check 和三个 exact test 都被继承
#563 的 `hyper-util 0.1.20` 不存在 `runtime` feature 阻断并返回 exit 101，三个 filter 均 matched/executed/ignored
`0/0/0`，不能记为通过。环境没有 `DATABASE_URL`、`psql` 或 `pg_isready`，两个真实 DB case 未执行；其 optional
fixture 即使 harness 可启动也会明确 self-return，不能凭进程 exit 0 误报 contract PASS。production router、事务
commit 后 publish 只有静态确认。direct rustfmt failure 与继承 resolver blocker 已交独立 fixer；review 仍异步，
PR 保持 Ready，原始失败记录不得删除。

独立 reviewer 锚定 implementation HEAD `3e8fff848d9cb9929502fb8309a1058205b28e47` 完成只读审查：无 P0，
有 3 个 P1 与 3 个 P2。P1：optional DB tests 缺/坏 `DATABASE_URL` 时成功 return，会产生零执行假绿；create
writer-first 由测试自己的 shared holder 阻塞，删除 production create lock 仍可通过，batch 也没有 race；Rust
`issue_status::resolve` 吞掉 storage error，导致 DB 故障被错误映射成 400/409 而 Go 返回 500。P2：reorder 的
malformed UUID/category/foreign/built-in/archived/cross-category status/error JSON 与 Go 不兼容且只测 omission；catalog
update 无返回行当前 404 而 Go concurrent guard 是 409，且新测试未调用 update；多数 catalog case 直接调用私有
handler，绕过 production router/middleware/extractor，TestFlags 也未断言真实 key/default。reviewer 确认唯一
production mount 复用同一 HandlerState/pool/bus/service/query、无 Stub/Noop/Fake/alternate path，单文件无新依赖方向
成立，但 635 行测试证据强度不足，不能支持完整契约或 Go 下线声明。全部 finding 已交独立 fixer，PR 保持 Ready。

## 53. [~] AUDIT-002 执行缺口：issue create admission and column ordering transaction contract

当前切片继续选择 `AUDIT-002`，把台账 §5 已点名但尚无 production behavior smoke 的 `internal/issueguard` 与
`internal/issueposition` 作为同一条 issue-create 事务能力迁移，不按两个小模块或 helper 拆 PR：

- Go `internal/issueguard/duplicate.go`、`internal/issueposition/position.go`、`service/issue.go` 和
  `handler/issue_create_position_test.go` 共同定义完整契约。标题用 Unicode whitespace collapse + lowercase 归一；
  active duplicate identity 是 workspace/project/parent/normalized-title；done/cancelled effective category 不阻塞，
  active built-in 或 custom-category issue 阻塞；`allow_duplicate` 仍必须取得同一 advisory transaction lock 后放行，
  避免与普通 create 竞态绕过。
- 两个同 identity 的真实 `IssueService::create` 并发时 advisory xact lock 必须序列化，最多一个普通 create 成功，
  loser 返回包含现有 issue id/identifier/title/status 的 typed duplicate outcome；不同 workspace/project/parent/title
  互不串扰。autopilot recent guard 还要按 autopilot/project/title/window 隔离，并在非正 window/空 title/无
  autopilot ID 时明确 no-op。
- 同一 create transaction 在 duplicate guard 后以 `(workspace,status)` 的当前 `MIN(position)-1` 插入，新 issue
  必须严格排在该列顶部；不同 status/workspace 独立，显式负 position 后仍从真实 minimum 继续，HTTP create 与
  autopilot create 复用各自已有 production caller，不能只测试 `next_top_position` helper。

Rust 默认生产链已存在：`cordy-handler::issue::create_issue` 调用唯一 `HandlerState.issues`，
`cordy-service::IssueService::create` 在同一 SQL transaction 内依次调用
`issue_guard::lock_and_find_active_duplicate` 和 `issue_position::next_top_position` 后插入；autopilot service 复用
recent guard/position。当前 Rust guard 只有纯函数/lock-key unit tests，position 只有 optional-DB helper test，无法
证明真实 create transaction、并发序列化、effective custom category、typed HTTP outcome 或 caller wiring。

本切片必须复用既有 handler/state/IssueService/query/advisory lock，不新增 repository、duplicate service、position
allocator、mock transaction 或 test-only production seam；用可证伪的真实 Postgres production caller checks 覆盖
上述完整事务。要求 DB 的 contract 不得在缺失/坏 `DATABASE_URL` 时成功 self-return；环境不可用必须由 verifier
如实报告为未执行/失败。fixture 必须用唯一 workspace 并显式清理。

- 默认生产路径：Rust `cordy-server` 的唯一 issue POST router 与 autopilot service 使用同一 DB/query/guard/position
  机制，不存在 Stub、Noop、Fake 或 alternate production allocator。
- 退出证据：删除任一 production duplicate lock/find 或 position caller 会使直接 contract 失败；独立 verifier
  负责真实 DB/compile/test，reviewer 对照 Go，finding 仅交 fixer。
- Go 是否可下线：本契约和异步 finding 收口后，Go issueguard/issueposition 回归不再是 Rust production 的依赖；
  AUDIT-002 其余能力与 AUDIT-001..010 仍未完成。
- owner：主 agent 只迁移完整契约与 Ready PR；独立 verifier/reviewer/fixer 异步。branch
  `codex/cord-230-issue-create-admission-order-rust`，依赖 Ready #565 branch at `5c81b6d5`。

实现 commit `44d533ab` 只修改两个既有 production module 内的 `cfg(test)`，没有新增文件、依赖、repository、
allocator 或生产 seam：

- `IssueService::create` 真实 transaction checks 覆盖连续列顶 position、显式负 minimum、status 列隔离、Unicode
  whitespace/case duplicate、typed existing row、allow_duplicate、done effective-category 放行、custom active category
  阻塞，以及 workspace/project/parent identity 隔离；
- 可证伪并发 case 先锁住 create 必经的 workspace counter row，让第一个 production create 在取得 duplicate
  advisory lock 后停住、第二个同 identity create 排队；释放后断言一个成功、一个 typed duplicate 且数据库只有
  一行，删除 production advisory lock 会让两个 lookup 都越过并最终插入；
- recent-autopilot guard 用真实 agent/autopilot/issue/run rows 验证 origin/project/title/window/active-run 查询、空 title/
  无 ID/非正 window no-op 和过期窗口；真实 issue POST router 验证 201、409 `active_duplicate_issue` wire payload、
  existing issue identity、allow_duplicate 与返回 position。

所有 DB contract 都要求 `DATABASE_URL` 并在缺失/坏连接时直接失败，不成功 self-return；fixture 使用唯一 workspace
并在正常路径显式清理。主 agent 只执行 `git diff --check`（PASS），没有运行 cargo/rustfmt/test。非 Draft Ready
PR #566 已创建，base 是 #565 branch；独立 verifier/reviewer 已异步派发，fixer 尚无 finding。不能据此声称已验证
或删除 Go。

## 54. [~] AUDIT-002 执行缺口：user WebSocket authenticated session contract

当前切片选择 `AUDIT-002` 的 user-facing WebSocket/realtime session，不与 daemon wakeup WS 或已核对等价的 Redis
event envelope 重叠：

- Go `cmd/server/router.go` 的 `/ws` mount 与 `internal/realtime/hub.go`/`hub_test.go` 定义完整会话：按
  `workspace_id` 或 slug 解析；同源/可信代理/allowlist origin gate；有效 `cordy_auth` cookie 在 upgrade 前完成 JWT/PAT
  与 membership；无 cookie 时第一条有界 JSON frame 必须是 auth token，成功回 `auth_ack`，失败回 `auth_error` 并
  close；超限 frame fail-closed。
- 已认证连接自动加入 workspace/user scope；task/chat subscribe 必须经过 DB ownership authorizer，返回
  `subscribe_ack` 或 `subscribe_error`，workspace/user/global 等禁止手工越权；unsubscribe 返回 ack，ping 返回 pong；
  断线后 client/scope rooms 和 subscriber lifecycle 必须清理，广播不能泄漏到 foreign user/workspace。
- Rust `cordy-handler::ws::ws_handler`、`post_upgrade`、read/write pumps、`DbPatResolver`、`DbScopeAuthorizer` 与
  `cordy-realtime::Hub` 已在唯一 `cordy-server` router 接线；但当前 `ws.rs` 只有 origin-policy unit tests，无法直接
  证明真实 HTTP upgrade、两种 auth branch、membership、wire frames、DB scope ownership和 disconnect cleanup。

本切片必须复用现有 handler/state/hub/auth/membership/task service，不新增 WebSocket server、auth service、hub、mock
router 或 test-only production seam；在既有 Rust module 内启动真实 loopback Axum production route，以真实
PostgreSQL fixture 和真实 WebSocket client执行完整会话矩阵。需要 DB 的 contract 缺失/坏 `DATABASE_URL` 必须失败，
不得成功 self-return；fixture 用唯一 user/workspace/member/task/chat rows并显式清理。网络/sandbox 若限制 loopback，
由 verifier 如实记录，不能用 helper unit test替代 production claim。

- 默认生产路径：`cordy-server` 只挂载这一 `/ws` handler 和同一 production Hub/authorizer；有效 cookie/PAT/JWT 与
  workspace membership 走真实 DB/cache，不存在 Stub/Noop/Fake/alternate hub。
- 退出证据：真实 loopback client 覆盖 upgrade 前拒绝、first-frame auth、ack/error、自动 scope、authorized/denied
  subscribe、ping/pong、广播隔离和 disconnect cleanup；删除 production auth/membership/authorizer/register/unregister
  任一环节会使 contract 失败。
- Go 是否可下线：本契约与异步 finding 收口后，Go user WS session 回归不再是 Rust production 依赖；Redis relay、
  background worker、AUDIT-001..010 的剩余退出门仍未完成。
- owner：主 agent 只迁移完整契约和 Ready PR；独立 verifier/reviewer/fixer 异步。branch
  `codex/cord-231-user-websocket-session-rust`，依赖 Ready #566 branch at `1bf44476`。

实现 commit `a8de8250` 只在既有 `ws.rs` 的 `cfg(test)` 增加真实 loopback contract，并给 handler tests 复用 workspace
已有的 `tokio-tungstenite` dev dependency；没有新增生产 server、router、hub、auth seam 或运行时依赖：

- 用 `build_router_from_state` 顶层 production assembly 启动真实 Axum listener，因此同一测试会安装真实
  `DbScopeAuthorizer`、CORS/request middleware、`ws_handler`、Hub 和 HandlerState services；
- 用真实 user/workspace/member/PAT/agent/issue/task rows 覆盖 foreign Origin upgrade 前 403、非 auth 首帧 error+close、
  有效 PAT 但非 member 的 auth_error+close、member PAT 第一帧 `auth_ack`，以及 workspace slug + cookie 的 upgrade 前
  auth branch；
- 已认证 session 直接断言 Hub 自动 workspace/user rooms、own workspace idempotent ack、foreign workspace deny、owned
  task DB authorization ack、unsubscribe ack/room removal、unknown task deny、application ping/pong、workspace broadcast wire、
  close 后 connections 与 task room 清理；
- `DATABASE_URL` 缺失/坏连接直接失败而非 self-skip；正常路径在 server graceful shutdown 后显式删除 task/issue/agent/
  PAT/member/workspace/users。sandbox loopback 或共享 DB 失败必须由 verifier 原样报告。

主 agent 只运行 `git diff --check`（PASS），没有运行 cargo/rustfmt/test，也没有机械重算 Cargo.lock；非 Draft Ready
PR #567 已创建，base 是 #566 branch。独立 verifier/reviewer 已异步派发，必须核对新增 dev dependency 是否要求
lock update并执行 exact loopback test；fixer 尚无 finding。当前不能声称 WS contract 已验证或删除 Go。

独立 verifier 在 exact Ready SHA `3a493196d7b876e27ed8bb4d81a87ccb6d84435a` 上确认 worktree、祖先关系、diff
和原 lock hash 检查通过，但发现新增 handler dev dependency 未写入 `Cargo.lock`，且新增测试 direct rustfmt 失败；继承自
#563 的非法 `hyper-util/runtime` feature 令 locked metadata、no-run、server/Windows build 和 exact WS test 全部 exit 101，
因此 exact test 是 0/0/0。环境没有 `DATABASE_URL`/PostgreSQL，DB、loopback 和 network contract 均未执行；目前只静态
确认 production assembly，不能把本次验证记为通过。

独立 reviewer 同一 exact SHA 无 P0，报告 2 个 P1、3 个 P2 和 1 个 P3：缺失 lock update；未认证连接未注册便轮询
connections，导致 auth error 后 close 断言是假阳性，且主动 unsubscribe 后才 close，不能证明 disconnect 清理自动 rooms；
所谓完整矩阵缺少 JWT、frame limit、chat、foreign broadcast/user isolation；loopback 没有复现 production ConnectInfo，
origin env precedence 与 Go 不同且受 ambient env 影响；失败路径不会关闭 server 或清理 DB fixture；325 行单体测试耦合过重
却仍缺矩阵。review 同时确认 required-DB 不会 self-skip、唯一 production `/ws` 使用真实 Hub/authorizer/HandlerState，未发现
Stub/Noop/Fake 或 alternate hub。上述 finding 已异步派发给独立 fixer；在修复及重验完成前，Go user WebSocket contract
不能下线，PR 保持 Ready。

## 55. [~] AUDIT-002 执行缺口：scheduler distributed worker lifecycle contract

当前切片继续台账既定的 `AUDIT-002 background worker smoke`，选择 Go `internal/scheduler` 的完整 DB-backed worker
契约，而不是再补一个局部 helper：

- Go `scheduler.Manager` 在 server 启动时注册 task-usage rollup 与 Autopilot schedule jobs，立即 tick；每个
  `(job, scope, plan_time)` 通过 `sys_cron_executions` 唯一约束和 lease token 保证多副本只有一个执行者，写入完整 audit
  row，并在失败后按同一 plan 重试。
- abandoned RUNNING 必须先标 FAILED；允许 reentry 时可带新 lease/attempt reclaim，旧 owner 的 heartbeat/terminal write
  必须失败，防止 stale runner 覆盖新 owner；handler panic、timeout、root cancellation 都必须写 FAILED 分类，正常 shutdown
  必须有界 join。
- Rust 唯一生产入口是 `cordy-server::build_production_router` 创建并注册两个真实 job 后调用
  `cordy_scheduler::Manager::start`，server drain HTTP 后 cancel root 并 `ManagerRuntime::shutdown`；实现已有但只有 planner/
  classifier unit tests，没有真实 PostgreSQL claim、lease theft、retry、audit payload、immediate tick 和 shutdown contract，
  因而不能证明 Go scheduler worker 可退休。

本切片必须直接执行既有 `Manager`/`ManagerRuntime`、真实 `sys_cron_executions` 表和 production SQL，不新增 scheduler、
queue、lease service、mock DB 或 test-only production seam。完整 evidence 至少覆盖 concurrent managers single winner、
success audit、failure same-plan retry、stale close/reclaim与旧 lease fencing、panic/timeout/cancel分类、start immediate tick和
bounded shutdown；删除 claim uniqueness、lease-token terminal guard、retry cursor、root cancellation 或 production job register/
start 任一环节应使 contract 失败。required DB 缺失/坏 `DATABASE_URL` 必须失败，不得 self-skip；fixture 使用唯一 job/scope
并 failure-safe 清理。

- 默认生产路径：Rust server 已只启动这一 DB-backed scheduler manager，无 Stub/Noop/Fake 或 alternate scheduler。
- Go 是否可下线：本 worker contract 及异步 finding 收口后，Go scheduler manager 回归可退出；两个 job 的业务副作用、
  其他 background workers 和 AUDIT-001..010 总退出门仍需分别完成。
- owner：主 agent 迁移完整契约和 Ready PR；独立 verifier/reviewer/fixer 异步。branch
  `codex/cord-232-scheduler-worker-contract-rust`，基于 #567 branch at `97edeaa3`。

实现 commit `f0ac870a` 在 `cordy-scheduler` 内增加一个 DB contract test module，直接调用现有 production
`Manager`/`ManagerRuntime` 与 `sys_cron_executions` SQL；除 `cfg(test)` module 声明外没有修改生产代码、依赖或新增
runtime seam：

- 两个真实 manager 以不同 runner 并发 claim 同一 job/scope/plan，断言唯一 handler、一个 success/一个 conflict，以及
  SUCCESS audit 的 attempt、owner、rows affected、JSON result、finished time；另由 `register` + `start` 证明进程 runtime
  启动后立即 tick，并通过 `shutdown` 有界 join。
- 同一 plan 第一次 handler error、第二次 success，断言 FAILED retry cursor 实际被 attempt 2 reclaim，终态清除 retry/error；
  独立真实 rows 覆盖 handler panic、run timeout 与 root cancellation 的 `handler_panic`/`run_timeout`/`canceled` 分类。
- stale owner 在 handler 内阻塞，测试把真实 lease 过期；第二 manager 的完整 tick 先 close stale row 再以新 lease/attempt
  reclaim，旧 owner 随后完成时必须得到 `LeaseLost`，不能覆盖新 owner 的 SUCCESS audit。
- required `DATABASE_URL` 缺失或 PostgreSQL 不可达直接失败；每个测试使用 UUID job prefix，正常路径显式删除，panic/failure
  由 Drop guard best-effort 异步清理。没有 fake DB、mock lease、sleep-based concurrency gate或 alternate scheduler。

主 agent 只执行 staged `git diff --check`（PASS），没有运行 cargo、rustfmt、测试或 DB 命令。production server 中两个真实
job 的注册、唯一 `Manager::start` 和 drain 后 `ManagerRuntime::shutdown` 已静态定位；exact compilation、matched/executed
counts、真实 migrated DB、server/Windows build 与 failure-safe cleanup 行为必须由独立 verifier 执行并如实记录。当前不能
声称 scheduler contract 已验证或删除 Go。

非 Draft Ready PR #568 已创建，base 是 #567 branch；Ready SHA `e058e8b4`。独立 verifier/reviewer 已异步派发，
fixer 尚无本 PR finding。PR 可在异步验证、review、fix 期间保持 Ready，主迁移线不等待。

## 56. [~] AUDIT-002 执行缺口：runtime heartbeat batching worker contract

当前切片继续 `AUDIT-002 background worker smoke`，选择 Go `handler.HeartbeatScheduler` 的完整热路径与 shutdown
契约；它是 runtime liveness/sweeper 的生产前置，而不是单个时间格式或 helper：

- 已 online 且有 `last_seen_at` 的 heartbeat 必须只入 pending set，同一 runtime 高频 schedule 在窗口内去重，并由单次
  PostgreSQL batch UPDATE 刷新；不同 runtime 同批处理，队列大小受 fleet ID 数量限制。
- offline、never-seen，以及 online touch 与 sweeper offline race 必须同步走 passthrough/`mark online`，让调用返回时状态
  已恢复；batch flush 时刚变 offline 的 row 不得被错误翻回 online，其下一次 heartbeat 必须自愈。
- production `cordy-server` 将同一个 `BatchedHeartbeatScheduler` 同时注入 HTTP heartbeat handler 并启动 worker；关闭时
  先 drain HTTP，再 cancel root，runtime 必须 flush pending、join，并额外 flush cancellation 后到达的 late schedule，不能
  在进程退出时丢最后一批 liveness。

Rust production 实现已在 `cordy-handler::heartbeat_scheduler` 与 `cordy-server` 接线，但没有直接测试；现有 Go DB tests
覆盖 coalesce、多 ID batch、offline fallback、race-to-offline self-heal、Stop drain/late schedule。当前切片必须在既有 Rust
module 内直接执行真实 `agent_runtime` PostgreSQL rows、`PassthroughHeartbeatScheduler`、`BatchedHeartbeatScheduler` 和
`HeartbeatSchedulerRuntime`，不得新增 scheduler、queue、mock DB、sleep gate或 test-only production seam。required DB
缺失/坏 `DATABASE_URL` 必须失败且 fixture failure-safe 清理。

- 退出证据：真实 DB contract 同时证明 coalesce/batch、sync fallback/race recovery、batch offline preservation、root cancel
  final flush、cancel 后 late schedule second drain与 bounded join；删除 production pending dedupe、status gate、fallback、batch
  SQL、final/second flush 任一环节会失败。
- 默认生产路径：唯一 Rust server assembly 复用同一 scheduler instance，无 Stub/Noop/Fake/alternate heartbeat worker。
- Go 是否可下线：本契约及异步 finding 收口后，Go heartbeat batching worker 回归可退出；runtime sweeper、其他 workers与
  AUDIT-001..010 总退出门仍未完成。
- owner：主 agent 迁移完整契约和 Ready PR；独立 verifier/reviewer/fixer 异步。branch
`codex/cord-233-heartbeat-worker-contract-rust`，基于 #568 branch at `3d448d16`。

实现 commit `35fbba80` 只在既有 `heartbeat_scheduler.rs` 的 `cfg(test)` 内增加 212 行真实 DB contract，没有修改
production scheduler、SQL、依赖、文件边界或新增 runtime seam：

- 两个 online runtime 记录一日前 timestamp，同一 ID schedule 两次、另一 ID 一次，直接断言 pending set 是 2 且 DB
  未提前写；`flush_once` 后 pending 清空且两 row timestamp 前进，证明 coalesce 与 multi-ID batch。
- offline/never-seen snapshot 直接经 production fallback 同步恢复 online；另把 online snapshot 的真实 row 先改 offline，
  证明 passthrough touch miss 会继续 mark online。batch race 则先 schedule online snapshot、再把 DB row 改 offline，flush 不得
  错误翻回；下一次用 fresh offline row schedule 必须同步自愈。
- shutdown case 在真实 runtime row 上持有 `FOR UPDATE` lock，让 root cancellation 的第一次 flush 已 drain pending但阻塞
  于 DB；此时再 schedule 第二个 late ID，释放锁后断言 `HeartbeatSchedulerRuntime::shutdown` 有界返回 Stopped、second flush
  清空 pending且两个 timestamp 都前进。该 gate 不靠 sleep，删 final flush或 shutdown 后 second flush会直接失败。
- required `DATABASE_URL` 缺失/坏 PostgreSQL 直接失败；fixture 用唯一 workspace/runtime rows，正常路径显式等待删除，Drop
  仅作为 best-effort panic cleanup，不把它扩大声明为 failure-safe。

主 agent 只执行 staged `git diff --check`（PASS），没有运行 cargo、rustfmt、测试或 DB 命令。production server复用同一
`BatchedHeartbeatScheduler` 注入 handler并启动 runtime的入口已静态定位；exact compile、matched/executed counts、真实 DB
竞争、server/Windows build和清理行为由独立 verifier执行。当前不能声称 heartbeat worker 已验证或删除 Go。

非 Draft Ready PR #569 已创建，base 是 #568 branch，Ready SHA `be2bc21d`；独立 verifier/reviewer 已异步派发，
fixer 尚无本 PR finding。PR可在异步结果期间保持Ready，主线不等待。

## 57. [~] AUDIT-002 执行缺口：runtime sweeper stale-liveness/offline contract

当前切片继续台账既定的 background-worker smoke，选择 runtime sweeper 中一个完整、可独立验收的生产行为面：

- Go `sweepStaleRuntimes` 每轮从 `agent_runtime` 找出超过 150 秒未 heartbeat 的 online candidates；Redis liveness 可用且
  报告存活时必须保留 online，Redis 不可用/超时则 fail-open 到 DB stale 判定；确认 offline 后清理 liveness key，并按唯一
  workspace 发布 `daemon.register` 的 `stale_sweep` 事件。
- Rust `RuntimeTaskSweeper::sweep_stale_runtimes` 已接入唯一 `run_once`/`run_full_once` production worker，并由
  `cordy-server` 在真实 pool、Redis-or-DB liveness 与 shared Bus 上启动；但当前只有常量/clock unit test，没有真实
  `agent_runtime` rows、liveness filtering、DB race/offline transition、forget 和 broadcast evidence，不能据此退休 Go。

本切片必须复用现有 `RuntimeTaskSweeper`、`LivenessStore`、runtime SQL、`Bus` 和 `cordy-server` wiring，不新增 sweeper、
queue、liveness service、mock production router 或 alternate event path。required `DATABASE_URL` 缺失/坏连接必须失败，
fixture 使用唯一 workspace/runtime rows并在 failure path 可清理。contract 至少覆盖 stale dead→offline、stale alive→保留、
liveness unavailable→DB fallback、online/race update 不误广播、liveness forget以及同 workspace 单事件；删除 stale cutoff、
liveness gate、DB status guard、forget或Bus publish任一环节应使测试失败。

- 默认生产路径：Rust `RuntimeTaskSweeper::run_full_once` 是唯一 server sweeper，使用同一 production liveness store、TaskService
  与 Bus；有效配置不选择 Stub/Noop/Fake。测试中的 liveness double 只用于明确的 unavailable/alive 判定分支。
- Go 是否可下线：本 stale-runtime contract 和异步 finding 收口后，Go stale runtime sweep 回归可退出；offline task failure、
  reconnect retry、stale/queued task cleanup、delegated recovery、chat finalize、runtime GC 与 AUDIT-001..010 总门仍未完成。
- owner：主 agent 迁移完整契约和 Ready PR；独立 verifier/reviewer/fixer 异步。branch
  `codex/cord-234-runtime-sweeper-contract-rust`，基于 #569 branch at `b24d14bb`。

实现 commit `07fce06c` 只在既有 `runtime_sweeper.rs` 的 `cfg(test)` 内增加 256 行真实 DB contract，没有修改
production sweeper、runtime SQL、依赖、Bus 或 liveness seam：

- 用唯一 workspace 和多条真实 `agent_runtime` rows 证明 150 秒 stale cutoff：dead stale row 变 offline，alive stale row
  由 liveness gate 保持 online，fresh/已 offline row不受影响；确认 offline 的 row 触发一次该 workspace 的
  `EVENT_DAEMON_REGISTER`/`{"action":"stale_sweep"}`，并调用 liveness `forget`。
- liveness unavailable 分支直接验证回退到 DB stale 判定；另由 liveness double 在 candidate 查询后把 row 改 offline，证明
  online→offline TOCTOU 时 conditional UPDATE 返回空、不广播且不虚报 sweep count。
- 测试直接调用 production `RuntimeTaskSweeper::sweep_stale_runtimes`（该函数由唯一 `run_once`/`run_full_once` worker 调用），
  不新建 mock router、alternate event path 或 production fake；测试 liveness double 仅表达真实 Redis alive/unavailable/race
  三个外部状态。
- required `DATABASE_URL` 缺失/坏 PostgreSQL 直接失败；正常路径显式删除唯一 workspace，Drop 仅 best-effort cleanup，不把
  panic cleanup扩大声明为failure-safe。

主 agent 只执行 staged `git diff --check`（PASS），没有运行 cargo、rustfmt、测试或 DB 命令；production server 的唯一
`RuntimeTaskSweeper::start`/shared liveness/Bus wiring已静态定位。exact compile、matched/executed counts、真实 DB/liveness
行为、server/Windows build与失败清理由独立 verifier执行。当前不能声称 stale-runtime contract 已验证或删除 Go。

非 Draft Ready PR #570 已创建，base 是 #569 branch，当前 Ready SHA `7258dccabaf839af8dc1a38cd8b06c803a064627`；PR
可在异步验证、review、fix 期间保持 Ready，主线不等待。

独立 verifier 在 exact HEAD `7258dccabaf839af8dc1a38cd8b06c803a064627` 上确认 worktree、祖先关系、range diff-check
和 `Cargo.lock` 未变化检查通过；直接对新增 `runtime_sweeper.rs` 执行 stable rustfmt check 失败，报告了测试行的调用包裹、事件
payload 断言、unavailable sweep 断言、raced fixture 和最终 race 断言等格式差异。继承自 #563 的非法
`hyper-util 0.1.20` `runtime` feature 使 locked/offline metadata、handler no-run、server/Windows build 和 exact
runtime-sweeper filter 均 exit 101；exact filter 在 harness 前 `matched=0, executed=0, ignored=0, passed=0, failed=0`。
环境没有 `DATABASE_URL`、`pg_isready` 或 `psql`，required PostgreSQL fixture 因而未执行；不能把 DB、liveness、事件、forget、
server/Windows 或 cleanup 行为写成通过。verifier 静态确认唯一 server sweeper assembly 仍使用真实 pool/liveness/TaskService/Bus，
但没有运行时证据。

独立 reviewer 在同一 exact HEAD 报告无 P0、2 个 P1、3 个 P2：DB contract 直接调用私有
`sweep_stale_runtimes`，没有锁住真实 `run_once`/server worker 边界；TOCTOU double 直接把候选改成 offline 而不是刷新
`last_seen_at`，且未订阅 race Bus/保留 forget recorder，不能证明 heartbeat race 下不广播/不 forget；只有一条 dead row，
同 workspace 单事件去重不可证伪；unavailable 与 configured-store error 未分开且 unavailable 分支按 Go 语义不应 forget；
`Drop` 异步清理不是确定性的 failure-path cleanup。reviewer 确认 production assembly 使用真实 shared state，未发现有效生产路径
误选 Stub/Noop/Fake。上述 verification/review finding 已交独立 fixer；在修复并重新验证前，本契约不能声称已验证或删除 Go。

## 58. [~] AUDIT-002 已登记执行缺口：offline task failure and reconnect-retry terminal contract

本项在开始编码前登记，选择 runtime sweeper 中共享 reconnect-grace 的完整任务恢复能力，而不是只增加一个 SQL
helper 或把整套 sweeper 混入同一 PR：

- Go `sweepOfflineRuntimeTasks` 必须在 runtime 已 `offline` 且超过配置的 reconnect grace 后，按每 tick 上限把
  `dispatched`、`running`、`waiting_local_directory` 任务原子地置为 `failed`，写入
  `runtime_offline`、完成时间、错误和清空 wait reason；grace 内不能误杀，并交由 `TaskService.HandleFailedTasks`
  统一广播、issue/agent reconcile 与 retry 副作用。
- Go `sweepExpiredRuntimeReconnectRetries` 必须只处理 runtime_offline parent 产生的过期 `deferred` retry；健康且新鲜
  的 runtime 重连时保留 retry，runtime 未在完整 grace 内恢复时写入 `runtime_reconnect_timeout` 的终态，避免 issue
  永久卡住并使 runtime GC 可收敛。两个 stage 共享 bounded batch、`FOR UPDATE SKIP LOCKED` 和 reconnect-grace 顺序，
  但不把 dispatched/running backstop、queued TTL、chat finalize 或 runtime GC 的不同语义伪装为本项完成。
- Rust 唯一生产入口是 `RuntimeTaskSweeper::run_once` 的 offline-task stage 与 reconnect-retry stage，server 仍通过
  唯一 `run_full_once` worker 调用；实现已存在但当前只有 SQL/常量，没有真实任务状态、grace、健康重连 race、terminal
  failure side-effect 和 production run_once contract 证据。

本切片必须复用既有 `RuntimeTaskSweeper::run_once`、`cordy-db` production SQL、`TaskService` 和 shared `Bus`，不新增
task failure service、retry queue、fake DB 或 alternate sweeper。required `DATABASE_URL` 缺失/坏连接必须失败而不能
self-skip；fixture 使用唯一 workspace/user/member/agent/runtime/issue/task lineage，并在正常和 failure path 确定性清理。
contract 至少覆盖：grace 内保留与超 grace 后三种 active task 状态批量失败、batch cap/row lock 安全、runtime_offline
retry 在健康 runtime 下保留、离线超 grace 后 `runtime_reconnect_timeout` 终止、parent lineage 过滤、terminal failure
经真实 `TaskService` 触发 issue/agent/event 收口；删除 status predicate、grace gate、runtime freshness race、parent
lineage、failure reason 或 production `run_once` wiring 任一环节应使测试失败。

- 默认生产路径：Rust `RuntimeTaskSweeper::run_full_once` 是唯一 server sweeper，offline/reconnect stage 使用真实
  `PgPool`、production `TaskService` 与 shared `Bus`；有效配置不选择 Stub/Noop/Fake。
- Go 是否可下线：本 offline-task/reconnect-retry contract 与异步 finding 收口后，这两段 Go sweeper 逻辑可退休；
  stale/queued task cleanup、delegated recovery、chat finalize、runtime GC、其余 background workers 与 AUDIT-001..010
  总退出门仍未完成。
- owner：主 agent 迁移完整契约、生产入口和 Ready PR；独立 verifier/reviewer/fixer 异步。计划 branch 基于
  `codex/cord-234-runtime-sweeper-contract-rust`，独立 worktree，编号待创建后回写。

实现 commit `e7b6a3d1` 只在既有 `runtime_sweeper.rs` 的 `cfg(test)` 内增加真实 PostgreSQL recovery contract，没有修改
production stage、SQL、TaskService、Bus 或新增 runtime seam：

- 唯一 fixture 创建 workspace/user/member、三个 runtime（offline 超 grace、offline 在 grace 内、online+fresh）、对应
  agents/issues 和完整 task lineage。三种 active 状态在超 grace 后都由真实 `RuntimeTaskSweeper::run_once` 终止，grace 内的
  running task 保留；失败 row 断言 `runtime_offline`、错误、完成时间和清空 wait reason。
- 一个 `runtime_offline` parent 的 deferred retry 在 offline runtime 超 grace 后由同一 `run_once` 终止为
  `runtime_reconnect_timeout`；healthy+fresh runtime 的 retry 与非 runtime_offline parent 的 retry 均保留，证明健康重连
  race 和 parent lineage gate。
- 真实 `TaskService::handle_failed_tasks` 通过 shared Bus 收口 task failure/issue update，并断言 old agent 归 `idle`、grace
  agent 仍为 `working`；额外持有一个 task 的 PostgreSQL `FOR UPDATE` 锁，以 production SQL 的 `SKIP LOCKED` 和 `max_per_tick=1`
  直接证明限额及锁安全，再恢复该 row 后运行完整 worker。
- required `DATABASE_URL` 缺失/坏连接直接失败；契约使用唯一 UUID fixture，测试正常和失败返回路径显式删除 workspace/user，
  Drop 仅作为 best-effort 兜底，不把环境缺失写成通过。

主 agent 仅执行 staged `git diff --check`（PASS），没有运行 cargo、rustfmt、测试或 DB 命令。非 Draft Ready PR #571 已创建，
base 是 `codex/cord-234-runtime-sweeper-contract-rust` 的 `78b04074`，当前 branch
`codex/cord-235-offline-task-recovery-contract-rust`，Ready SHA `e7b6a3d1`。独立 verifier/reviewer 已异步派发；在 exact
compile、matched/executed counts、required DB、production server/Windows 和 failure cleanup 证据返回前，本契约不能声称已
验证或删除 Go，PR 保持 Ready。

## 59. [~] AUDIT-002 已登记执行缺口：stale dispatched/running and queued task cleanup contract

本项在开始编码前登记，选择 runtime sweeper 中另一条完整任务清理能力；它与 §58 的 offline/reconnect terminal path
共享 `TaskService` 收口但有独立的 liveness、lease、wall-clock 和 queued-lineage 语义：

- Go `sweepStaleTasks` 必须把超 dispatch/run wall-clock 的任务置为 `failed/timeout`，但 dispatched 的有效
  `prepare_lease`、online+fresh runtime heartbeat 和 running runtime 在 reconnect grace 内必须保留；runtime 缺失、
  lease 过期或 heartbeat 超过 reconnect grace 才可进入 backstop，`waiting_local_directory` 不得被误杀。
- Go `sweepExpiredQueuedTasks` 必须只清理超过两小时 TTL 的 `queued` rows，使用 bounded `FOR UPDATE SKIP LOCKED`；
  与 `runtime_offline` parent 关联的 deferred/retry lineage 不得被通用 queued TTL 误杀，抢占/claim race 在 apply-time
  status+TTL predicate 下必须安全收敛。终止 rows 交由 `TaskService.HandleFailedTasks`，保留 issue/agent/event/retry
  一致性。
- Rust 唯一生产入口是 `RuntimeTaskSweeper::run_once` 中的 `fail_stale_tasks` 与 `expire_stale_queued_tasks`
  stages，server 仍由唯一 `run_full_once` worker 调用；实现和 production SQL 已存在，但当前没有真实任务状态、
  prepare lease、runtime freshness/reconnect gate、queued retry exemption、claim race 和生产 `run_once` side-effect
  evidence。

本切片必须复用既有 `RuntimeTaskSweeper::run_once`、`cordy-db` production SQL、`TaskService` 与 shared `Bus`，不新增
task cleanup service、queue、fake DB 或 alternate worker。required `DATABASE_URL` 缺失/坏连接必须失败而不能 self-skip；
fixture 使用唯一 workspace/user/member/agent/runtime/issue/task lineage，并在正常和 failure path 确定性清理。contract 至少
覆盖：有效 lease/fresh runtime 保留、expired lease/absent-or-stale runtime timeout、running reconnect-grace gate、
waiting-local-directory 保留、queued TTL bounded cleanup、runtime_offline retry exemption、claim/cleanup `SKIP LOCKED`
竞态、`timeout`/`queued_expired` reason/error/completion、真实 `TaskService` 的 issue/agent/event 收口；删除对应 gate、
lineage predicate、batch cap、apply-time guard 或 production `run_once` wiring 任一环节应使测试失败。

- 默认生产路径：Rust `RuntimeTaskSweeper::run_full_once` 是唯一 server sweeper，stale/queued stages 使用真实 `PgPool`、
  production `TaskService` 与 shared `Bus`；有效配置不选择 Stub/Noop/Fake。
- Go 是否可下线：本 stale/queued cleanup contract 与异步 finding 收口后，Go 两段 cleanup 逻辑可退休；delegated recovery、
  chat finalize、runtime GC、其他 background workers 与 AUDIT-001..010 总退出门仍未完成。
- owner：主 agent 迁移完整契约、生产入口和 Ready PR；独立 verifier/reviewer/fixer 异步。计划 branch 基于
  `codex/cord-235-offline-task-recovery-contract-rust`，独立 worktree，编号待创建后回写。

实现 commit `480105aa` 只在既有 `runtime_sweeper.rs` 的 `cfg(test)` 内增加真实 PostgreSQL cleanup contract，没有修改
production stages、SQL、TaskService、Bus 或新增 worker seam：

- 唯一 fixture 将既有 rows 置为 terminal 后创建 fresh runtime、stale-but-alive runtime、过期/有效 prepare lease 的
  dispatched rows、长时间 running row、fresh-runtime running row、`waiting_local_directory` row，以及两个普通 queued
  rows 和一个 `runtime_offline` parent queued retry。真实 `RuntimeTaskSweeper::run_once` 证明 stale dispatched/running
  只在 timeout/liveness gate 允许时失败，lease/fresh running/waiter 保留；queued rows 以 `queued_expired` 终止，retry
  lineage 保留。
- 在生产 SQL 调用前持有一个 queued row 的 PostgreSQL `FOR UPDATE` 锁，以 `max_per_tick=1` 直接证明 `SKIP LOCKED` 和
  bounded batch，再恢复该 row 后通过完整 `run_once` 让真实 `TaskService::handle_failed_tasks` 收口四个终止任务。
- 订阅 shared Bus 并断言每个 workspace-scoped task failure 的 `timeout`/`queued_expired` reason/error，另验证 task issue
  与 agent 状态副作用；liveness double 只报告 stale runtime alive，验证 stale DB heartbeat 不被误判为 daemon 死亡。
- required `DATABASE_URL` 缺失/坏连接直接失败；契约使用唯一 UUID fixture，正常和失败返回路径显式删除 workspace/user，
  Drop 仅作为 best-effort 兜底，不把环境缺失写成通过。

主 agent 仅执行 staged `git diff --check`（PASS），没有运行 cargo、rustfmt、测试或 DB 命令。非 Draft Ready PR #572 已创建，
base 是 `codex/cord-235-offline-task-recovery-contract-rust` 的 `cce8b6b1`，当前 branch
`codex/cord-236-stale-task-cleanup-contract-rust`，Ready SHA `480105aa`。独立 verifier/reviewer 已异步派发；在 exact
compile、matched/executed counts、required DB、production server/Windows 和 failure cleanup 证据返回前，本契约不能声称已
验证或删除 Go，PR 保持 Ready。

## 60. [~] AUDIT-002 已登记执行缺口：runtime GC transactional lifecycle contract

本项在开始编码前登记，选择 runtime sweeper 中最后一条可独立收口的 runtime GC 业务能力；它不是对前两项 task
cleanup 再加几个断言，而是完整覆盖“候选发现→每 runtime 行锁→资格重检→未排空任务保护→历史解绑→删除→workspace
事件”的事务生命周期：

- Go `gcRuntimesWithBudget` 以 7 天 offline TTL、blocked gauge 上限和每 tick bounded candidates 扫描；每个候选在独立
  5 秒操作预算内以 `FOR UPDATE` 锁定 runtime，重新确认 offline/stale/unbound，检查所有未完成 task，只有 drained runtime
  才解绑 terminal task history、fail-closed 确认没有残留引用并删除 runtime；一个候选失败或超时不得中止同 tick 其他候选。
- 并发 enqueue 必须与 runtime 行锁协调：写入方拿不到已删除 runtime 的 owner fence 时失败，不能在 GC commit 后留下孤儿 task；
  blocked runtime、active agent、fresh/online runtime、非 terminal history 和 runtime 消失等 apply-time 竞态必须安全保留。
- 删除成功后按 workspace 去重发布 `EVENT_DAEMON_REGISTER` 的 `{"action":"runtime_gc"}`，metrics/gauge/error/budget
  结果不能被弱化为只看 SQL helper 的单元自返回。
- Rust production implementation 已存在于 `RuntimeTaskSweeper::gc_once`/`gc_with_budget`/`gc_runtime`，并由唯一
  `run_full_once` worker 调用；但当前没有真实 PostgreSQL contract 证明事务回滚、所有 non-terminal status 保护、owner-lock
  enqueue race、bounded candidate/blocked count、tick/operation budget、terminal history 保留、workspace event 去重和
  production full-sweeper wiring。因此 Go GC 逻辑仍不能退休。

本切片必须复用既有 `RuntimeTaskSweeper`、`cordy-db` production SQL、`TaskService`、shared `Bus` 和现有唯一 sweeper
入口，不新增 GC service、fake transaction、alternate deletion path 或通用测试框架。required `DATABASE_URL` 缺失/坏连接
必须失败而不能 self-skip；fixture 使用唯一 workspace/user/member/agent/runtime/task lineage，正常和 failure path 都
确定性清理。contract 至少覆盖：terminal message/usage/token history 保留且解绑、每种 non-terminal task status 阻止删除、
active agent/online/fresh runtime 排除、bounded candidate 与 blocked gauge、删除失败事务 rollback、runtime `FOR UPDATE`
与 concurrent enqueue owner fence、每 tick budget、candidate 失败隔离、dedup workspace event，以及真实 `run_full_once`
返回的 GC report；删除任何 status/agent/task guard、transaction boundary、lock/race、budget 或 event assertion 应使测试失败。

- 默认生产路径：Rust `RuntimeTaskSweeper::run_full_once` 是唯一 server sweeper，GC stage 使用真实 `PgPool`、production
  `cordy-db` queries、shared `Bus` 和 metrics；有效配置不选择 Stub/Noop/Fake。
- Go 是否可下线：本 runtime GC transactional contract 与异步 finding 收口后，Go `gcRuntimes` 生命周期可退休；delegated
  failure recovery、chat finalize、其他 background workers 与 AUDIT-001..010 总退出门仍未完成。
- owner：主 agent 迁移完整契约、生产入口和 Ready PR；独立 verifier/reviewer/fixer 异步。计划 branch 基于
  `codex/cord-236-stale-task-cleanup-contract-rust`，独立 worktree，编号待创建后回写。

实现 commit `cb70bb98` 只在既有 `runtime_sweeper.rs` 的 `cfg(test)` 内增加 runtime GC 的真实 PostgreSQL contract，未
修改 `gc_once`/`gc_with_budget`/`gc_runtime` production path、SQL、TaskService、Bus 或 server worker wiring：

- 唯一 `GcRows` fixture 创建 workspace/user/member、fresh helper runtime/agent，并为每个场景创建唯一 offline runtime；
  两个 drained runtime 通过真实 `run_full_once` 被删除，terminal completed/failed/cancelled task 的 message、usage、token
  全部保留，`runtime_id` 显式解绑，且同 workspace 的两次删除只发布一个 `runtime_gc` daemon-register event。
- 五种 non-terminal status（queued/dispatched/running/waiting_local_directory/deferred）逐一阻止 `gc_runtime`；active bound
  agent、fresh offline runtime、online runtime 同样保留；blocked-count query 观察到全部五个 blocked runtime，bounded
  candidate query 以 `max_per_tick=1` 不超过上限。
- owner-lock contract 使用真实 PostgreSQL `FOR UPDATE` 与 `pg_stat_activity` wait evidence：writer 在 GC runtime 行锁期间被
  `lock_task_owner_rows` 阻塞，释放后成功 enqueue，随后 GC 重新检查未排空 task 并保留 runtime；drained runtime 删除后相同
  owner-fenced enqueue 返回空且不产生孤儿 task。
- required `DATABASE_URL` 缺失/坏连接直接失败；正常和 failure 返回路径显式解绑 agent、删除 workspace/user，Drop 仅为
  best-effort 兜底。shared DB 可能含其他旧候选，因此 full-sweeper 断言只要求本 fixture 的两个 runtime 被删除，并按本
  workspace 过滤事件。

主 agent 仅执行 staged `git diff --check`（PASS），没有运行 cargo、rustfmt、测试、DB 或长编译命令。非 Draft Ready PR #573
已创建，base 是 `codex/cord-236-stale-task-cleanup-contract-rust` 的 `b335afa4`，当前 Ready tip `85c3e2fa`（实现
`cb70bb98`）；独立 verifier/reviewer 已异步派发，fixer 待 finding 返回后异步交付。exact compile、matched/executed
counts、required DB、server/Windows 和 timeout/rollback 证据返回前，本契约不能声称已验证或删除 Go，PR 保持 Ready。

## 61. [~] AUDIT-002 delegated failure recovery durable outbox contract

本项在开始编码前登记，选择 runtime sweeper 中 delegated failure recovery 的完整业务能力：终态 delegated task 生成平台
恢复信号、持久化 comment outbox、coordinator dispatch/merge、崩溃后 bounded replay、用户取消/手动 rerun 语义、三次未
送达后的 exhaustion，以及禁止 recovery task 自递归唤醒。它与 runtime GC、queued cleanup 的状态机和事务边界不同，不能
用一个“comment 存在”断言代替整条恢复契约：

- Go `TaskService.RecoverPendingDelegatedFailures` 与 `runtime_sweeper.go` 负责每 tick bounded replay；`ensure...` 只为
  eligible final delegated failure 创建一个 system `progress_update` comment，comment 创建成功但 dispatch 失败时必须能
  由 sweeper 再次发现并补发，不能重复 comment/task。
- coordinator 有 queued/pending、dispatched、running 三种覆盖语义：pending task 合并最新 signal 并保留旧 coalesced
  comment；dispatched task 只登记 planned-but-undelivered，completion reconcile 再创建 successor；running task 必须创建
  独立 successor。retry-pending、autopilot、backlog/done/cancelled source issue、archived/unbound/self source agent、普通
  failure 和 recovery task 本身都必须 fail closed，不得形成循环。
- recovery task 的用户取消与 manual rerun 必须区分：显式用户取消会把 comment 记入 `delivered_comment_ids` 并结束 obligation；
  manual rerun 保持信号可 replay。最多三次 undelivered coordinator attempts 后，原 recovery tasks 不再新增，创建一次可见
  exhaustion system comment 和 action-required inbox，并保持后续 sweep 幂等。
- Rust production implementation 已存在于 `cordy-service::task_recovery` 的
  `ensure_delegated_failure_recovery_comment`、`dispatch_delegated_failure_recovery`、
  `recover_pending_delegated_failures`，并由 `TaskService::handle_failed_tasks` 与唯一
  `RuntimeTaskSweeper::run_once` 接线；但当前没有真实 PostgreSQL contract 覆盖上述 outbox、lineage、dispatch race、
  exhaustion、side effects 和生产 sweeper wiring，因此 Go recovery service 仍不能退休。

本切片必须复用既有 `TaskService`、`cordy-db` production SQL、shared `Bus`、issue/agent/task/comment/inbox models 和
`RuntimeTaskSweeper::run_once`，不新增 recovery service、fake DB、alternate dispatcher 或测试框架。required `DATABASE_URL`
缺失/坏连接必须失败而不能 self-skip；fixture 使用唯一 workspace/user/member/同 runtime 的 coordinator+worker agent、
source/worker issue 与 task lineage，正常和 failure path 确定性清理。contract 至少覆盖：final failure comment/content redaction
和 issue/event side effect、committed-comment/no-task replay、duplicate/idempotent replay、pending merge、dispatched planned
follow-up、running successor、retry/autopilot/source guard、user-cancel acknowledgement、manual rerun replay、三次 exhaustion
comment/inbox 与 no-fourth-task、bounded `max_per_tick`/error aggregation、recovery self-recursion guard，以及真实 sweeper
`run_once` stage；删除任一 lineage/status/attempt predicate、transaction lock、coverage check、event/inbox assertion 或
production wiring 应使测试失败。

- 默认生产路径：Rust `RuntimeTaskSweeper::run_full_once` 的 `run_once` stage 调用真实 `TaskService` recovery；有效配置不选择
  Stub/Noop/Fake。
- Go 是否可下线：本 delegated failure recovery durable-outbox contract 与异步 finding 收口后，Go recovery service/sweeper
  逻辑可退休；chat finalize、其他 background workers 与 AUDIT-001..010 总退出门仍未完成。
- owner：主 agent 迁移完整契约、生产入口和 Ready PR；独立 verifier/reviewer/fixer 异步。计划 branch 基于
  `codex/cord-237-runtime-gc-contract-rust`，独立 worktree；实现、PR 与验证事实在下方回写。

实现 commit `82b51570` 在既有 production recovery path 上增加完整 PostgreSQL contract，未新增 recovery service、fake DB
或 alternate dispatcher；同时在既有 `RuntimeTaskSweeper` 测试模块增加真实 worker assembly 覆盖：

- `cordy-service::task_recovery` 的 fixture 创建唯一 workspace/user/member、同 runtime 的 coordinator/worker、source/worker
  issue 与 task lineage。真实 `TaskService::handle_failed_tasks` 断言终态 delegated failure 只生成一个 system
  `progress_update`、错误中的 API key 被 redacted、source/evidence/delegated-from lineage、comment/task event 与重复调用幂等。
- outbox replay 覆盖 comment 已提交但没有 task、bounded `max_per_tick=0`、重复 sweep、queued coordinator 合并最新 signal
  并保留旧 coalesced comment、dispatched planned-but-undelivered 的 completion successor、running coordinator 的独立 successor，
  以及 retry-pending、backlog、unbound source agent、普通 failure 与 recovery self-recursion 的 fail-closed guards。
- 用户取消通过真实 `cancel_task_by_user` 断言 `delivered_comment_ids` acknowledgement 后不 replay；manual rerun 通过真实
  `rerun_issue` 断言取消 pending recovery row 时不伪造 delivery receipt、信号仍可被 sweeper replay；三次 undelivered attempt
  后只创建一次 exhaustion system comment/action-required inbox，不产生第四个 recovery task。
- `cordy-handler::RuntimeTaskSweeper::run_once` 的 production assembly 使用真实 `PgPool`、`TaskService`、shared `Bus` 和固定时钟
  replay 同一 outbox，断言 report 的 `recoveries_replayed` 与 task-queued event；required `DATABASE_URL` 缺失/坏连接直接失败，
  正常和失败路径显式删除 workspace/user，Drop 仅作 best-effort 兜底。

主 agent 仅执行 `git diff --check`（PASS），没有运行 cargo、rustfmt、测试、DB 或长编译命令。非 Draft Ready PR #574 已创建，
base 是 `codex/cord-237-runtime-gc-contract-rust` 的 `5d4532af`，当前 Ready tip `82b51570`；独立 verifier/reviewer/fixer
已异步派发。exact compile、matched/executed counts、required DB、server/Windows、取消/rerun、failure cleanup 和完整 sweeper
运行证据返回前，本契约不能声称已验证或删除 Go，PR 保持 Ready。

## 62. [~] AUDIT-005 deferred cancelled chat finalization contract

本项在开始编码前登记，选择 daemon cancel-ack 与 runtime sweeper fallback 共同完成的完整延迟聊天取消最终化能力；它不是在
已有 `TaskService` 方法外再加一个定时器，而是要证明“取消时延迟判定 → transcript flush/超时边界 → 原子 claim → restore 或
Stopped outcome → durable draft restore/event → session 删除竞态”的端到端契约：

- Go `CancelTask` 对已启动且 transcript 为空的 direct chat 只设置 `chat_finalize_deferred_at`，不提前删除用户输入；daemon
  cancel-ack 或 sweeper 在 60 秒 grace 后通过 `FinalizeDeferredCancelledChat` 决定仍为空时删除输入并在同一事务写入
  `chat_draft_restore`，有 transcript 时保留输入并写一条 `Stopped.` assistant message；channel-ingested 输入即使 session
  已归档/解绑也必须 fail closed，不能伪造可恢复草稿。
- finalizer 必须先锁 `chat_session` 再 claim task marker，与 workspace/agent/session deleter 的锁顺序一致；并发 ack/sweeper
  只能有一个 winner，事务失败必须回滚 marker 让下一个 tick 可重试，session 已消失时清 marker 但不得插入孤儿 restore；事件
  `chat:cancel_finalized` 只携带 outcome/metadata，不泄露 prompt，广播丢失仍可由 draft-restore endpoint 恢复，restore consume
  幂等。
- sweeper 查询必须只返回超过 grace 的 marker，使用 bounded batch；fresh marker 不得提前 finalize，重复 sweep/重复 ack 不得创建
  第二条 assistant message、第二个 restore 或第二个事件；生产 `RuntimeTaskSweeper::run_once` 必须报告实际处理的 rows，并
  继续执行同一 tick 的其他 stages。

Rust production implementation 已存在于 `TaskService::finalize_deferred_cancelled_chat`、`agent::list_chat_finalize_deferred_expired`
与 `RuntimeTaskSweeper::run_once`，daemon cancel-ack 已接入同一 TaskService；但当前 Rust 没有与 Go
`task_cancel_finalize_test.go`/`chat_draft_restore_race_test.go` 等价的真实 PostgreSQL contract，不能据此退休 Go。

本切片必须复用既有 `TaskService`、`cordy-db` production SQL、`RuntimeTaskSweeper::run_once`、shared `Bus` 与 chat draft-restore
endpoint，不新增 finalizer service、alternate timer、fake DB 或测试框架。required `DATABASE_URL` 缺失/坏连接必须失败而不能
self-skip；fixture 使用唯一 workspace/user/member/runtime/agent/session/task/message/attachment，并在 setup 成功和失败路径
确定性清理。contract 至少覆盖：cancel 未启动/已启动非空/已启动空 transcript 三分支、grace 内外 bounded query、仍为空
restore（含附件分离、broadcast 丢失恢复与幂等 consume）、late transcript 的 Stopped outcome、atomic marker claim 与重复调用、
channel-ingested archived/unbound guard、session-delete/lock-order race、事务失败回滚可重试、事件 payload redaction、真实
`RuntimeTaskSweeper::run_once` 的 `chats_finalized` report 和同 tick stage isolation；删除 grace/status/provenance predicate、
lock order、transaction boundary、idempotence、cleanup 或 production wiring 任一环节应使测试失败。

- 默认生产路径：Rust `RuntimeTaskSweeper::run_full_once` 是唯一 server sweeper，`run_once` 使用真实 `PgPool`、production
  `TaskService`、shared `Bus` 和 daemon cancel-ack；有效配置不选择 Stub/Noop/Fake。
- Go 是否可下线：本 deferred chat-finalization contract 与异步 finding 收口后，Go `FinalizeDeferredCancelledChat`、marker
  sweeper stage 与对应 cancel-ack wiring 可退休；其他 background workers、delegated recovery、runtime GC 与 AUDIT-001..010
  总退出门仍未完成。
- owner：主 agent 迁移完整契约、生产入口和 Ready PR；独立 verifier/reviewer/fixer 异步。计划 branch 基于
  `codex/cord-238-delegated-failure-recovery-contract-rust`，独立 worktree；实现、PR 与验证事实将在本节回写。

实现 commit `8782170f` 只在既有 `cordy-handler::runtime_sweeper` 的 `cfg(test)` 内增加真实 PostgreSQL contract；production
`TaskService`、marker/lock/restore queries、Bus、daemon cancel-ack 与 `RuntimeTaskSweeper::run_once` wiring 未改动：

- `ChatFinalizeRows` 用单一 setup transaction 创建唯一 user/workspace/member/runtime/agent/session/task/user message/attachment；
  setup 出错自动 rollback，测试结果和 cleanup 路径显式删除无 FK 的 `chat_draft_restore` 后再删除 workspace/user。
- contract 覆盖未启动 queued chat 的同步 restore、started non-empty 的同步 `Stopped.`、started empty 的 deferred marker；验证
  60 秒 grace 内外查询、正数 `max_per_tick=1` 上限、真实 `run_once` 的 `chats_finalized` report、restore attachment detach、
  broadcast payload 不带 prompt、重复 sweep/claim 幂等、late transcript、channel-ingested guard 和 session 已消失时清 marker。
- session `FOR UPDATE` 由独立事务持有，finalizer 在锁释放前保持 marker，释放后再完成 restore，证明与 deleter 的锁顺序不会绕过
  session fence；第二个 expired marker 用生产查询证明批次 cap，而不是只断言单行结果。

主 agent 仅执行 `git diff --check`（PASS），没有运行 cargo、rustfmt、测试、DB 或长编译命令。非 Draft Ready PR #575 已创建，
base 为 `codex/cord-238-delegated-failure-recovery-contract-rust` 的 `e7b268b9`；原始实现 commit 为 `8782170f`，随后 fixer 提交
`840261fe` 修正事务提交与删除错误传播，后续 fixer commit `a9fdd2e1` 收口 report、并发、HTTP 和 channel contract，当前 PR
tip 为 `a9fdd2e1`；独立
verifier/reviewer/fixer 已异步派发。exact compile、matched/executed counts、required PostgreSQL、server/Windows、取消/ack、
session deletion race、failure cleanup 和完整 sweeper evidence 返回前，本契约不能声称已验证或删除 Go，PR 保持 Ready。

独立 review 在 exact tip `ac2c36b1` 发现 P0：sync 与 deferred finalizer 创建 transaction 后从未 commit，所有 restore/
Stopped/marker 写入都会在函数返回时 rollback，deferred path 甚至可能为回滚结果发布成功事件。fixer commit `840261fe`
为两个 finalizer 的 marker-only、missing row/session、restore、Stopped 全部成功/早退路径补 commit，且把两处被 `.ok()`
吞掉的 input-delete error 恢复为 error/rollback；主 agent 已机械推送到 PR #575。

后续 fixer follow-up 修正 report 语义：`finalize_deferred_cancelled_chat` 仅在 settlement commit 且有 outcome 时返回 true，
`run_once` 只累计 true，不再把 selected/no-op/error candidates 全算作 `chats_finalized`。contract 同 tick 注入一个无 session
候选并断言 marker 被清但 report 仍只计真实 restore；ack/sweeper 两个并发 finalizer 在真实 task row holder 后同时进入
PostgreSQL lock wait，释放后严格一 true/一 false且只生成一个 restore。draft restore 证据穿过 production Router，覆盖 creator
含 attachment fetch、non-creator GET/DELETE 403、两次 consume 204，以及真实 session DELETE 等待 concurrent writer commit
后清除无 FK restore。channel fixture 现在实际创建 binding、archive session 再 unbind，并验证 immutable provenance；event
同时精确约束 workspace/task/session/system actor 与 allowed payload keys。

fixer 环境 `DATABASE_URL` unset，required PostgreSQL contracts 未执行；locked/offline no-run 仍在 discovery 前被 inherited
#563 `hyper-util 0.1.20` 不存在 `runtime` feature 阻断（exit 101，实际 0 tests）。fixed-stable rustfmt 对 fixer 触及的
`chat_api.rs`/`runtime_sweeper.rs` PASS，`git diff --check` PASS；在 resolver 修复传播且有 migrated PostgreSQL 前不能把上述
race/HTTP/commit evidence 登记为 executed PASS。

## 63. [~] AUDIT-007 Go 高风险回归契约映射索引（T-53）

本项在开始补测试前登记，目标是把 807 个 Go 测试按业务契约归档，而不是按文件名机械翻译。索引只承认可定位的 Rust
contract/production entry；仅有 route parity、类型能编译或 Rust 文件存在，不能标记为等价。每行必须区分“已有 Rust 证据”、
“当前 PR/台账切片待 verifier”与“尚需新增 contract/不适用理由”，并把 wire/schema/ID 的细节转交 AUDIT-008，避免两个
台账 ID 重复计数：

| 风险域 | Go 回归来源（代表性，不声称穷尽） | Rust 证据/入口 | 当前状态 | 退出动作 |
| --- | --- | --- | --- | --- |
| API、auth、permission、错误 JSON | `server/internal/handler/*_test.go`、`middleware/*_test.go`、`daemon_auth_test.go` | `server-rs/scripts/route_parity.py`；`cordy-handler` 的 route/validation/error contract；`cordy-auth` JWT/Redis tests；Rust production router | 部分覆盖；route method/path 不是响应/权限/事务等价证明 | 为高风险 handler 补 response/auth/permission/error-envelope smoke；wire 字段转 AUDIT-008 |
| DB transaction、locking、rollback | `server/internal/service/*_test.go`、`handler/*race_test.go`、scheduler lock tests | #565/#566 issue transaction contracts；#567 WS session；#568/#569 workers；#571/#573/#575 sweeper contracts；`cordy-db` production SQL | 已按完整业务切片登记/提交，真实 DB 与异步 finding 未全部收口 | 每个切片记录 required DB、并发/rollback、matched/executed 与 cleanup 证据；缺口回到对应 AUDIT-002/005 |
| provider、integration、fail-closed | `server/internal/integrations/**/*_test.go`、provider client/credential tests | PR #532..#541 channel/provider production contracts；各 crate 的 credential/config guards；`cordy-server` channel runtime | 生产 wiring 已有，真实凭证/网络 smoke 仍待 verifier | 为每 provider 记录正/负向矩阵、Stub/Noop 只在测试或 fail-closed 的理由；不适用项写明外部依赖 |
| daemon、task lifecycle、concurrency | `server/internal/daemon/**/*_test.go`、`daemonws/**/*_test.go`、task terminal/retry tests | PR #542..#563 daemon lifecycle contracts；`cordy-daemon::ProductionStack`、task execution、control/heartbeat；#565..#575 task workers | 部分覆盖；长生命周期/跨平台/真实 daemon 进程仍未完成 | 继续按 registration→claim→execute→reconcile→shutdown 业务链补 contract，不拆成按文件 PR |
| security boundary、redaction、secret handling | `server/internal/util/secretbox/*_test.go`、`middleware/auth_test.go`、provider redaction tests | `cordy-service::redact`、`cordy-agent::command` redaction、`cordy-auth` JWT、#574 recovery payload redaction | 核心静态/单元证据已有；Unicode/control、真实日志和外部凭证边界仍需审计 | 补不泄露 secret 的 wire/log vectors；安全 finding 交独立 reviewer/fixer，不能以“字符串相等”代替 |
| backfill、migration、CLI/exit code | `server/internal/*backfill/**/*_test.go`、`cli/*_test.go`、`migrate/*_test.go` | `cordy-migrate` runner/backfill bins；`cordy-cli::error` exit-code tests、CLI command contract tests；PR #518..#523/#555 | CLI/parser 多数已有 Rust unit contract；新鲜 DB/image/operator recovery 未完全执行 | 记录参数、退出码、锁/取消/恢复、镜像产物和 Windows/Linux evidence；发布入口仍归 AUDIT-001/006 |
| wire/schema、time、UUID/ULID、Redis/event envelope | `server/internal/*_test.go` 中 JSON/time/id/redis/event tests | 现有 protocol/event/serde tests、WS tests；route parity | 不在本索引重复宣布；属于 AUDIT-008 的独立兼容门 | 建 golden vectors、round-trip、旧数据读取与跨语言 event fixture；完成前不得删除 Go |
| 不适用或仅测试辅助 | `internal/testutil`、纯 UI/mock helper、仅 Go runtime 的测试 harness | Rust fixture/test helper 或无生产对应物 | 不把测试辅助文件当作缺失业务能力；若 helper 隐含契约，转入上面风险域 | 每项写出“不适用”理由和替代 Rust evidence，禁止用删除测试文件掩盖契约丢失 |

索引的首批落点是 #565..#575 与既有 #518..#563：它们按业务契约记录了 Go 来源、Rust 入口、默认生产状态和 Go 下线条件，
但 Ready PR 的存在不等于测试通过。AUDIT-007 的退出证据是所有高风险行都有可执行 Rust contract 或明确不适用理由，并能
回链到 AUDIT-002..006/008 的具体 PR、命令和异步 verifier/reviewer/fixer 结果；主 agent 不运行长测试，也不代做缺陷修复。

实现 commit `39532bfd` 只更新本台账：新增八个风险域映射行，分别回链代表性 Go 测试来源、Rust contract/production entry、
当前覆盖状态与下一步退出动作；明确 route parity 不等于行为等价、Ready PR 不等于测试通过、测试 helper 不等于生产缺口，
并把 wire/schema/time/UUID/ULID/Redis/event envelope 交给 AUDIT-008。没有新增生产代码、依赖、测试框架、Stub/Noop/Fake 或
默认入口。

主 agent 仅执行 `git diff --check`（PASS），没有运行 cargo、rustfmt、测试、DB、provider、daemon 或 release 命令。非 Draft
Ready PR #576 已创建，base 为 `codex/cord-239-chat-finalize-contract-rust` 的 `840261fe`，当前 tip 为 `96662713`；独立
verifier/reviewer/fixer 已异步派发。索引引用的各 Ready PR 仍须分别记录 exact command、matched/executed、环境限制和异步
finding；AUDIT-007 未完成前不能声称全部 Go 回归已映射或删除 Go。

## 64. [~] AUDIT-008 UUID/ULID wire serialization（T-54）

本项在开始编码前登记。`server-rs/crates/cordy-util::Ulid` 当前以 `uuid::Uuid` 作为内部值，但其 serde wrapper
仍输出 UUID 的连字符十六进制字符串；Go `oklog/ulid/v2` 的 `String()` wire contract 是 26 字符 Crockford
Base32。repo-wide 搜索确认当前没有 production caller，因此本切片只建立安全 codec prerequisite，不宣称已经改变事件 ID、
Redis envelope 或 API wire。

范围只收口这个已有 wrapper 的序列化/解析契约：复用 workspace 已有 `ulid` crate，按同一 16-byte value 转换为
26 字符 canonical ULID，并拒绝旧 UUID-hyphenated wire form；保留 `Ulid(uuid::Uuid)` 的现有 Rust 类型形状，
不新增 ID service、生成器或第二套 wire type。contract tests 使用固定跨语言向量、round-trip、长度/字符集和
旧 UUID 形式拒绝断言；事件/Redis 业务路径仍分别由 AUDIT-008 后续切片验证。

- 默认生产路径：当前没有 production field 使用 `cordy-util::Ulid`；直接调用 `ulid::Ulid::new().to_string()` 的 realtime
  事件路径不经过本 wrapper。未来接入者必须为具体 field 补 golden/round-trip，不能以 utility unit test 代替。
- Go 是否可下线：不能。本切片只移除 unsafe utility codec TODO；至少一个真实 production field 的兼容接入与对应旧数据/
  event/Redis/API 证据仍未完成，AUDIT-008 与 AUDIT-001..010 总退出门保持未完成。
- owner：主 agent 负责最小实现、生产可复用类型和 Ready PR；独立 verifier/reviewer/fixer 异步负责编译/contract
  验证、兼容性审查和缺陷修复。实现 commit、验证命令/结果、异步 finding 和 PR 会在本节追加；在此之前不得声称
  全部 wire 兼容已通过。

实现 commit `874d7493` 更新既有 `cordy-util::Ulid` serde wrapper 与锁文件：复用 workspace 已有 `ulid` crate，把内部
`uuid::Uuid` 的 16 bytes 编为 26 字符 Crockford Base32，并从该 wire value 还原 UUID；删除旧 UUID-hyphenated TODO。
contract tests 覆盖固定 Go 向量 `01ARZ3NDEKTSV4RRFFQ69G5FAV`、round-trip、canonical 字符集/长度和旧 UUID wire 拒绝。
主 agent 仅执行 `git diff --check`（PASS），没有运行 cargo、rustfmt、测试、DB 或长编译命令；非 Draft Ready PR #577 已创建，
base 为 `codex/cord-240-test-contract-matrix-rust` 的 `1486e6e3`，当前实现提交为 `874d7493`。异步 verifier/reviewer/fixer
结果待回写；在 exact compile、matched/executed、跨语言 event/Redis/旧数据读取和生产路径证据返回前，本项不能声称
AUDIT-008 已完成或删除 Go。

## 65. [~] AUDIT-009 backfill runbook Rust 入口对齐（T-55）

本项在开始编辑文档前登记。`server/cmd/backfill_issue_last_activity/README.md` 仍把运维命令写成从 `server/` 执行的
`go run ./cmd/backfill_issue_last_activity`，但对应能力已由 `server-rs/crates/cordy-migrate/src/bin/backfill_issue_last_activity.rs`
作为 Rust production binary 提供。旧说明会让 operator 在 Go 退休后走错误入口，也没有记录 Rust binary 与 `cargo run`
两种受支持调用方式。

范围只更新这份既有 backfill runbook：保留 migration 前置条件、批次/中断/重试和完成判据，改为仓库根目录下的 Rust
binary/`cargo run --locked` 入口，并明确 `DATABASE_URL` 与参数保持不变。无需新增脚本、发布资产、服务或第二套 backfill
实现；Docker/Makefile/release 的产物接线由 AUDIT-001/006 保持唯一来源。

- 默认生产路径：operator 优先执行已构建的 `server-rs/target/release/backfill_issue_last_activity`；源码路径只作为
  `cargo run --locked -p cordy-migrate --bin backfill_issue_last_activity` 的开发/恢复入口。
- Go 是否可下线：该 README 不再要求 Go toolchain，但 Go backfill source、其余 install/systemd/release/rollback 文档、
  新鲜产物验证和 AUDIT-001..010 总退出仍未完成。
- owner：主 agent 负责最小文档切换和 Ready PR；独立 verifier/reviewer/fixer 异步核对命令、参数、发布路径和回归，
  结果在本节追加；在此之前不得声称全部运维文档已切换。

实现 commit `72bbf9a2` 仅更新 `server/cmd/backfill_issue_last_activity/README.md`：从仓库根目录提供 Rust binary 和
Cargo 源码入口，保留既有 `DATABASE_URL`、参数、批次、中断/重试和 completion 判据。主 agent 仅执行 `git diff --check`
（PASS），未运行 cargo、backfill、PostgreSQL、Docker 或 release 命令；非 Draft Ready PR #578 已创建，base 为
`codex/cord-241-ulid-wire-contract-rust` 的 `979205c8`。异步 verifier/reviewer/fixer 结果待回写；文档切换不能替代
真实产物/运维验证，也不能据此删除 Go。

独立 reviewer 在 exact `d3a37ed0` 发现 upstream `ulid::Ulid::from_string` 会接受首字符 `8..Z` 并截断 130-bit
Crockford overflow，同时 repo-wide 只有 wrapper 定义/单元测试、没有 production caller。fixer follow-up 在 serde 入口先
强制 26 字符且首字符仅 `0..7`，再调用 crate parser；新增首字符 `8`/`Z`、非法 Crockford 字符、标点、25/27 长度和
canonical max `7ZZ...` vectors。台账据实降格为 utility-only prerequisite，删除 production/default-path 与 Go-downline
过度声明；真实 field 接入仍是 AUDIT-008 blocker。fixer 的 `rustfmt --config skip_children=true --check` 与
`git diff --check` PASS；locked/offline exact unit test 在 discovery 前被 inherited #563 `hyper-util 0.1.20` 不存在
`runtime` feature 阻断（exit 101，实际 0 tests），不能登记为 executed PASS。

## 66. [~] AUDIT-008 daemon event ID generator cutover（T-54A）

本项在开始编码前登记。Go `server/internal/daemonws/notifier.go` 使用 `ulid.Make().String()` 生成 daemon wakeup
event ID；Rust `server-rs/crates/cordy-daemon/src/notifier.rs::new_event_id` 当前自行复制 timestamp/random/Crockford
编码。两者 wire 形状相同但实现分叉，Rust 入口没有复用 workspace 已有 ULID 实现，后续修复容易产生跨进程兼容漂移。

范围只替换该 daemon notifier 的 ID 生成实现：复用 workspace 已有 `ulid::Ulid::new().to_string()`，删除手写编码和不再需要
的时间导入；保留现有 `String` API、事件 payload、Redis/本地 fanout、dedup 和错误语义。既有 notifier contract test 继续验证
26 字符 canonical Crockford、唯一性和所有 wakeup 事件路径；不新增 ID service、wrapper、生成器或测试框架。

- 默认生产路径：`cordy-daemon::RelayNotifier` 的 task-available、runtime-profile、workspace-change 和 pending-work
  事件统一经 `new_event_id` 使用 workspace `ulid` crate 生成，并继续走现有 local hub/Redis relay 入口。
- Go 是否可下线：该 daemon event ID 生成器已切到共享 Rust 依赖后，可标记此窄能力迁移；AUDIT-008 其余 event envelope/
  Redis 旧数据、AUDIT-002/005 生产验证和 AUDIT-001..010 总退出仍未完成，不能删除 Go。
- owner：主 agent 负责生产入口迁移、机械检查、提交和 Ready PR；独立 verifier/reviewer/fixer 异步验证 ULID 向量、编译和
  Redis/event 兼容并处理 finding，结果追加到本节。

实现 commit `f6a48f7e` 更新 `cordy-daemon` 的既有 `new_event_id` 生产 helper：为 daemon 增加 workspace `ulid` 直接依赖，
task-available、runtime-profile、workspace-change 和 pending-work 四类 public notifier 都经 `ulid::Ulid::new().to_string()`
生成 canonical event ID；删除手写编码但保留所有 relay、dedup、payload 和错误语义。该切片没有引入新的 wrapper 或 ID service。
主 agent 仅执行 `git diff --check`（PASS），未运行 cargo、rustfmt、测试、Redis、daemon 或 release 命令；非 Draft Ready PR #579
已创建，base 为 `codex/cord-242-backfill-runbook-rust` 的 `39325098`。异步 verifier/reviewer/fixer 结果待回写，在
exact compile、matched/executed 和真实 event/Redis 证据返回前，本项不能声称 AUDIT-008 已完成或删除 Go。

独立 verifier 在 exact HEAD `1d214da8` 核对四条 notifier 生产路径、依赖锁文件和 base ancestry：`git diff --check`、
`git diff --check 39325098...HEAD`、依赖静态一致性及 `rustfmt --edition 2021 --check notifier.rs` PASS。locked/offline
`cargo metadata`、daemon `cargo check`、`--no-run` 和精确 `event_ids_are_crockford_ulids` test 均在 discovery 前被
继承的 #563 `hyper-util 0.1.20` 不存在 `runtime` feature 阻断（exit 101；matched/executed 为 0），因此没有登记为
测试通过；Redis、daemon、发布和跨平台 smoke 未执行。

独立 reviewer 在 exact HEAD 确认 task-available、runtime-profile、workspace-change、pending-work 四条真实 notifier
路径都经 `new_event_id`，生产 `HandlerState`/server wiring 继续注入并调用该 notifier；手写 timestamp/random/Crockford
编码和无用时间导入已删除，锁文件只增加既有 `ulid` 的 daemon 直接依赖。Rust `Ulid::new()` 与 Go `ulid.Make()` 的
canonical 26 字符 Crockford wire shape/唯一性契约一致，但同毫秒 entropy 单调性不同；当前 event ID 仅作 dedup key、不作
排序，因此不扩大本切片声明。reviewer 唯一 P2 是本节重复标题，已在本提交删除重复段；fixer 尚无新增代码 finding。

## 67. [~] AUDIT-008 realtime/daemon ULID generator centralization（T-54B）

本项在开始编码前登记。#577 已把 `cordy-util::Ulid` 的 serde wire codec 对齐 Go 的 26 字符 Crockford ULID，#579 又在
daemon notifier 中新增了真实 `ulid::Ulid::new().to_string()` caller；但 `cordy-realtime` 的 node/event ID 生产路径仍在
多个 relay/broadcaster 文件中直接调用 `ulid`，daemon notifier 也保留独立直接依赖。这样同一 Go ULID wire contract 由多处
入口维护，后续很容易再次分叉。当前仓库已有 `cordy-util` crate，适合承载唯一的 Rust ULID 生成入口。

范围只新增 `cordy_util::new_ulid` 的薄封装，并将 `cordy-realtime` 的 Redis/sharded/mirrored/switchable
relay 以及 `cordy-daemon::notifier` 的生产 node/event ID 调用切换到该入口；保留现有 `String` API、26 字符 canonical
Crockford wire shape、dedup、payload、Redis 和错误语义。移除这些 crate 对 `ulid` 的重复直接依赖；不新增 ID service、
全局状态、随机生成器或第二套 wire type。

- 默认生产路径：`RedisRelay`、`ShardedStreamRelay`、`MirroredRelay`、`SwitchableRelayBroadcaster` 和 daemon
  `new_event_id` 都通过同一 `cordy_util::new_ulid()` 生成 node/event ID，server assembly 与 relay mode 不变。
- Go 是否可下线：仅 ULID 生成实现分叉可标记为收口；旧 Go notifier/realtime、跨语言 event/Redis golden、真实 loopback、
  旧数据读取及 AUDIT-001..010 总退出证据仍未完成，不能删除 Go。
- owner：主 agent 负责最小生产调用迁移、依赖锁文件、机械检查、提交/推送和 Ready PR；独立 verifier/reviewer/fixer 异步
  负责生成向量、调用覆盖、编译和回归 finding。

实现 commit `e7eeac1f` 在 `cordy-util` 增加薄的 `new_ulid` 生成入口，并把 `cordy-realtime` 的
`RedisRelay`、`ShardedStreamRelay`、`SwitchableRelayBroadcaster`、`MirroredRelay` 以及 daemon notifier 的所有直接
`ulid::Ulid::new().to_string()` 调用切到该入口；同时移除 realtime/daemon 的重复直接 `ulid` 依赖，保留现有 `String`
API、payload、dedup、Redis 和 relay wiring。docs commit `840ef87b` 将本仓库规则明确为 Rust 迁移默认只做 Rust
验证，Go 测试不作为门禁，且由独立 verifier 承担 cargo format/check/test/build。主 agent 仅执行 `git diff --check`
（PASS），没有运行 cargo、rustfmt、测试、Redis、daemon 或 release 命令；非 Draft Ready PR #580 已创建，以
`codex/cord-243-daemon-event-id-ulid`（base SHA `1bf33aca`）为 base，当前 tip 为 `068400ee`。独立 verifier/reviewer/fixer 已返回结果；在 exact compile、
matched/executed、跨语言 event/Redis/旧数据读取和真实生产 smoke 证据返回前，本项不能声称 AUDIT-008 已完成或删除 Go。

独立 reviewer 在 exact `970af5b6` 发现 P0：`new_ulid()` 调用 `Ulid::new().to_string()`，但 UUID-backed wrapper 没有
`Display`/`ToString`，代码在 workspace resolver 恢复后必然 E0599。fixer 保留既有 serde wrapper API，只让唯一薄入口直接
返回 `ulid::Ulid::new().to_string()`，并删除无用的 `Ulid::new`，没有扩张 typed wrapper surface。fixed-stable rustfmt 与
`git diff --check` PASS；locked/offline exact `generated_ulid_is_canonical_wire_value` 仍在 discovery 前被 inherited #563
`hyper-util 0.1.20` 不存在 `runtime` feature 阻断（exit 101，实际 0 tests），不能登记为 executed PASS。

独立 reviewer 在最终 exact HEAD `44318ea5` 确认原始 P0 已由 fixer 收口；全仓只剩 `cordy_util::new_ulid` 内一处直接
`ulid::Ulid::new()`，RedisRelay、ShardedStreamRelay、SwitchableRelayBroadcaster、MirroredRelay 和 daemon notifier 的
全部生产调用均经该入口，默认 assembly、relay wiring、依赖锁文件和 wire shape 无新增产品 finding。reviewer 另指出
本 PR 的 `AGENTS.md` 规则改动属于全仓治理范围；该改动是用户明确要求写入 agents.md 的迁移规则，故保留并在 PR 中明示，
没有伪装成 ULID 业务实现。历史 P0/P1/P2 均已在台账和 PR body 说明；AUDIT-008 仍受真实 compile、Redis/event 和生产 smoke
证据缺口约束。
独立 verifier 在最终 clean exact HEAD `068400ee` 确认 base `1bf33aca` 祖先关系、`git diff --check`（含 base range）和
依赖静态接线通过；触及文件 fixed-stable rustfmt 通过，但全 workspace `cargo fmt --all -- --check` 因继承格式漂移 exit 1。
locked/offline `cargo metadata`、`check`、`clippy`、`test --no-run`、debug/release build 以及三个精确 ULID test 均在
discovery 前被继承 #563 的 `hyper-util 0.1.20` 缺少 `runtime` feature 阻断（exit 101；matched/executed 为 0），不能登记
为编译或测试通过；Redis、daemon、release、跨平台 smoke 未执行，原因已记录。静态 inventory 确认 14 个生产 caller 全部
经 `cordy_util::new_ulid`，util 内仅一处直接 `ulid::Ulid::new()`。

## 68. [ ] AUDIT-008 Redis event envelope cross-language contract（T-54C）

本项在开始补契约前登记。Go `server/internal/realtime/redis_relay.go` 的 `envelope`、XADD field map、XREAD 解码和
`deliverEnvelope` 共同定义跨进程 event contract；Rust `cordy-realtime::Envelope`、`parse_xread_response` 和
`deliver_envelope` 已存在实现与局部 unit tests，但当前只断言 Rust 自己构造的数据，缺少固定 Go-shaped Redis field fixture、
缺失/空 payload 的 fail-closed 行为，以及 daemon-runtime/user/global/workspace 四种 scope 的 routing 证据。没有这些证据，
不能声称 Redis event envelope 已完成跨语言兼容。

范围只补既有 `cordy-realtime` envelope/parser/delivery contract：复用现有 `Envelope`、`parse_xread_response`、`HubFanout` 和
`DaemonRuntimeDeliverer`，加入固定 Go-compatible field/JSON fixture、Redis nested response malformed vectors、scope routing、
event_id injection 和 empty payload 断言；不新增 Redis client、event service、第二套 schema、生产 fallback 或测试框架。
生产 publish/consume wiring 和 Redis keys/fields 保持不变。

- 默认生产路径：`RedisRelay` 的 XREADGROUP、`ShardedStreamRelay` 的 XREAD、`Envelope::from_field_pairs` 与
  `deliver_envelope` 继续使用现有 server assembly；本切片只让跨语言契约可执行验证。
- Go 是否可下线：不能。该切片只补 event envelope contract evidence；真实 Redis/loopback、旧数据读取、provider/daemon smoke、
  其他 JSON/time/DB 兼容和 AUDIT-001..010 总退出仍未完成。
- owner：主 agent 负责最小完整契约迁移、测试 fixture、生产入口不变性、机械检查、提交/推送和 Ready PR；独立 verifier/reviewer/fixer
  异步负责编译、contract execution、跨语言审查和回归修复。

实现 commit `4b8d71d9` 在既有 `cordy-realtime::Envelope`、`parse_xread_response` 和 `deliver_envelope` 上补齐 Go-shaped
JSON/Redis fixture、固定字段 round-trip、重复字段 last-wins、空/缺失 payload fail-closed、malformed XREAD entry 丢弃、
event_id injection 以及 workspace/user/global/daemon-runtime 四种 scope routing；生产 publish/consume assembly、Redis
keys/fields 和 daemon deliverer wiring 未改变。主 agent 仅执行 `git diff --check`（PASS），没有运行 cargo、rustfmt、测试、
Redis、daemon 或 release 命令；Ready PR #581 将以 `codex/cord-245-realtime-ulid-wrapper`（base SHA `068400ee`）为 base
创建。异步 verifier/reviewer/fixer 结果待回写；在 exact compile、matched/executed、真实 Redis/loopback、旧数据读取和
生产 smoke 证据返回前，本项不能声称 AUDIT-008 已完成或删除 Go。
独立 verifier 在 exact `ed439ac2` 发现新增 `envelope.rs` test 未通过 fixed-stable rustfmt。独立 reviewer 另发现 P1：production
Redis URL 允许 RESP3，但 `parse_xread_response` 只接受 RESP2 `Value::Array`，会静默丢弃合法 `Value::Map`；P2：所谓固定
Redis fixture 由 Rust writer 自己生成后再交 Rust reader，且 routing harness 没有断言 workspace scope ID、user/global
exclude、frame 和 event ID；P3：该 harness 与 sharded relay 的弱版本重复。fixer 让同一 parser 同时接受 RESP2 array 与
RESP3 map，并增加真实 RESP3 top-level map vector；fixture 改为九个字面 Go field pairs，四个 routing branch 的全部参数均直接
断言，旧弱 harness 删除而不新增测试框架。fixed-stable rustfmt 与 `git diff --check` PASS；locked/offline `xread_parser_`
精确 filter 仍在 discovery 前被 inherited #563 `hyper-util 0.1.20` 不存在 `runtime` feature 阻断（exit 101，实际 0 tests）。
未执行真实 Redis/daemon smoke，不能把新增 direct contract 登记成 external loopback PASS。

随后将本分支重放到 #580 当前 tip `8387d2c7`：注册、实现、台账和 fixer 的有效 commit 分别为 `c70b394f`、`7e4073ff`、
`59bddde8` 和 `a21c6587`，当前 Ready PR #581 base 为 `codex/cord-245-realtime-ulid-wrapper`（`8387d2c7`）。重放未改变
生产代码；fixer 已关闭上述 P1/P2/P3，修复后的 touched-file rustfmt 与 `git diff --check` 均 PASS。既有 locked/offline
验证仍诚实记录为 inherited #563 resolver 在 discovery 前阻断、matched/executed 为 0，真实 Redis/daemon smoke 未执行；当前
AUDIT-008 仍不能标记完成或删除 Go。

## 69. [ ] AUDIT-008 realtime RFC3339Nano timestamp wire compatibility（T-54D）

本项在开始修改生产时间格式前登记。Go realtime envelope `CreatedAt` 与 heartbeat value 使用
`time.Now().UTC().Format(time.RFC3339Nano)`；Rust `cordy-realtime` 当前在 `Envelope::new`、legacy heartbeat 和 sharded
heartbeat 使用 `chrono::SecondsFormat::AutoSi`。`AutoSi` 只输出 0/3/6/9 位小数，而 Go 会去除所有尾随零（例如
`.123400000` → `.1234`），因此有效 Rust event/heartbeat wire value 仍可能与 Go 字符串不一致。

范围只为现有 `cordy-util` 增加一个无状态 `rfc3339_nano` helper，并切换 realtime envelope/heartbeat 的三个生产调用；
复用 `chrono`/现有 UTC 值，保留秒级 `rfc3339`、Redis keys、payload、TTL、relay assembly 和错误语义。contract tests 使用
固定 0/4/9 位 fractional vectors 与 UTC `Z` suffix；不新增 time service、timezone state、数据库迁移或 fallback。

- 默认生产路径：`Envelope::new`、`RedisRelay::heartbeat_once` 与 `ShardedStreamRelay::heartbeat_once` 继续由现有 server
  assembly 调用，仅 timestamp formatter 改为 Go-compatible RFC3339Nano。
- Go 是否可下线：不能。该切片只收口 realtime 时间字符串精度；handler/service/analytics/daemon 其他时间字段、真实 Redis/
  loopback、旧数据读取、生产 smoke 和 AUDIT-001..010 总退出仍未完成。
- owner：主 agent 负责最小 helper、生产调用和 Ready PR；独立 verifier/reviewer/fixer 异步负责 exact Rust 验证、跨语言
  timestamp review 和回归修复。

实现 commit `78c1bc41` 增加 `cordy-util::rfc3339_nano`，用 chrono 的九位纳秒格式再去除所有尾随零，覆盖 Go 的 0/4/9
位 fractional vectors；`Envelope::new`、`RedisRelay::heartbeat_once` 与 `ShardedStreamRelay::heartbeat_once` 三个 realtime
生产调用已切换到该 helper。Redis keys、payload、TTL、relay assembly 和错误语义未改变。主 agent 仅执行 `git diff --check`
（PASS），没有运行 cargo、rustfmt、测试、Redis、daemon 或 release 命令；Ready PR #582 以 `codex/cord-246-realtime-envelope-contract`
（base SHA `e4f92ada`）为 base，产品/fixer tip 为 `1cfc8ab9`，最终台账证据 commit 为 `41ada05a`。独立 fixer 的格式提交为
`1cfc8ab9`，fixed-stable rustfmt 与 `git diff --check` PASS；verifier 在最终 exact HEAD `41ada05a` 复核 base ancestry、clean
worktree、直接 touched-file rustfmt 和 base-range diff-check 均 PASS。locked/offline metadata/check/test/no-run 仍在继承 #563
`hyper-util` runtime resolver 错误前置阻断（exit 101，matched/executed 为 0），真实 Redis/daemon/release/cross-platform smoke
未执行。reviewer 无 P0/P1/P2/P3 finding。在 exact compile、matched/executed、跨语言 timestamp 和真实生产 smoke 证据返回前，
本项不能声称 AUDIT-008 已完成或删除 Go。

## 70. [ ] AUDIT-008 handler/service RFC3339Nano helper centralization（T-54E）

本项在开始修改 handler/service 时间输出前登记。Go 的 `server/internal/handler/comment.go` 游标、
`server/internal/handler/daemon.go` task message payload，以及 `server/internal/service/task.go` 的 chat/issue 通知
使用 `time.RFC3339Nano`；Rust 已迁移的 `cordy-handler::timefmt::rfc3339_nano` 与
`cordy-service::task_notify::rfc3339_nano` 却各自调用 chrono `AutoSi`，会把 `.123400000` 输出成 `.123` 而不是 Go 的
`.1234`。这会让 API header、task message payload、chat/issue event 和 autopilot timestamp 产生可观察的字符串差异。

范围只把已有 handler/service 的 RFC3339Nano 生产调用切到 T-54D 已验证的 `cordy-util::rfc3339_nano`，删除两份
`AutoSi` 本地 helper 和重复测试，保留现有 endpoint、JSON 字段、header 名称、事件 payload、排序/游标、配置、数据库和
错误语义；不扩展到 seconds-only `rfc3339`、第三方 provider 时间、数据库时间类型或新的时间服务。

- 默认生产路径：handler 的 comment cursor/task message 与 service 的 task notification/autopilot 仍由现有 server assembly
  调用，仅共享 formatter 实现改变；没有新增入口或 fallback。
- Go 是否可下线：不能。该切片只统一已迁移调用的时间字符串精度；其余 handler/analytics/daemon 时间字段、旧数据读取、
  真实 API/event smoke 及 AUDIT-001..010 总退出仍未完成。
- owner：主 agent 负责最小 helper centralization、生产调用和 Ready PR；独立 verifier/reviewer/fixer 异步负责 exact Rust
  验证、Go/Rust timestamp review 和回归修复。

实现 commit `93a45a58` 将 handler comment cursor/task-message payload 与 service task notification、issue/agent maps、autopilot
schedule key 的全部 RFC3339Nano 调用切到 T-54D 已验证的 `cordy-util::rfc3339_nano`，删除 handler/service 两份 `AutoSi`
helper；daemon task-message fixture 改为 `.123400Z` 并断言 Go-compatible `.1234Z`。seconds-only formatter、JSON/header/event
字段、排序、配置、数据库和错误语义未改变。主 agent 仅执行 `git diff --check`（PASS），没有运行 cargo、rustfmt、测试、DB、
API 或长编译命令；Ready PR #583 待创建，异步 verifier/reviewer/fixer 结果待回写。
