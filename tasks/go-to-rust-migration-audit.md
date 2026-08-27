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

| ID | 状态 | 已交付/当前切片 | 下一动作与退出缺口 | 依赖/可执行门 | 证据/PR | owner |
| --- | --- | --- | --- | --- | --- | --- |
| AUDIT-001 | 进行中 | 默认 server、CLI、migration、Docker、CI、Helm、CLI release 资产链、Desktop 内嵌 CLI、tag release 验证门、self-host exact-image rollback、opt-in systemd 生命周期与 required backend CI Go gate 已切到 Rust | 收口异步 finding；随后执行真实启动/升级/回滚演练 | release/installer/systemd/CI gate 已交付；最终生产验收依赖 AUDIT-002..009 退出 | PR #523/#527/#551..#554；详见 §11、§15、§16、§38..§41 | 主 agent；独立 V/R/F subagent |
| AUDIT-002 | 进行中 | route parity、CLI/daemon matrix、issue-status Ready #565 与 issue create admission/ordering Ready #566 已交付 | 异步收口 #565/#566 V/R/F，同时继续其他 API/WS/background worker smoke | 依赖现有 Rust IssueService/issueguard/issueposition production chain；#566 堆叠在 Ready #565 | PR #565/#566；§5、§6.2、§18、§52、§53 | 主 agent；独立 V/R/F subagent |
| AUDIT-003A | Ready PR | CPU/cmdline/symbol pprof 已接入；PR #556 的 Linux process telemetry 保留为趋势指标；PR #560 迁移真实 allocation-stack heap profile 与 Rust async runtime diagnostics | 异步收口 Cargo.lock、Linux/non-Linux/Docker 构建、真实 pprof/console client、public isolation、shutdown 与开销证据，finding 交 fixer | Rust server/profiling 入口可执行；依赖当前稳定 Rust、Linux release 构建和可写临时目录 | PR #524/#556/#560；详见 §12、§43、§47 | 主 agent；独立 V/R/F subagent |
| AUDIT-003B | Ready PR | logger 配置、TTY、component、request attrs 与本地毫秒时间布局已接入全部 Rust production subscriber | 异步验证真实输出、daemon rotating sink、timezone/DST与既有行为无回归，finding 交 fixer | Rust server/daemon/migrate/backfill 入口可执行 | PR #525/#557；详见 §13、§44 | 主 agent；独立 V/R/F subagent |
| AUDIT-003C | Ready PR | squad avatar 读写已接入既有 avatar capability | 等待异步 V/R/F，并纳入生产对象存储 smoke | 依赖 AUDIT-004 的生产存储证据完成退出 | PR #526；详见 §14 | 主 agent；独立 V/R/F subagent |
| AUDIT-003D | Ready PR | agent 的每实体限额已集中为默认 6、范围 1..50；daemon 的进程级 slot pool 独立保持默认 20、要求 >0 | 等待异步 V/R/F；生产 daemon 生命周期 smoke 继续归 AUDIT-005 | 配置契约可执行；最终退出依赖 AUDIT-005 daemon 生命周期 | PR #531；§6.2、§19 | 主 agent；独立 V/R/F subagent |
| AUDIT-004 | 主线切片已交付 | Lark、WeCom、DingTalk、Slack、Telegram、Composio、VCS、GHSnapshot 与 channel media production lifecycle 已交付 | verification 收口 supervisor/lease 矩阵、外部凭证 smoke/不可测原因与回滚策略；review/fix 异步回写 | 主 agent 当前无新的不重叠迁移缺口；最终退出依赖异步 V/R/F 直接证据 | PR #532..#536/#538..#541；§5.3、§6.2、§20..§28 | 主 agent；独立 V/R/F subagent |
| AUDIT-005 | 进行中 | `/health`、provider refresh、GC metadata、runtime/Remote/plugin-hook MCP、local-skills、wakeup/control、auto-update、poisoned-session、Codex rollout durability、confirmed provider demotion/recovery、private task temp 与 wakeup environment proxy production chain 已交付；当前切片迁移 heartbeat HTTP pool recovery | 接通连续 transient heartbeat failure 后的真实 idle-pool eviction 与新连接恢复；异步收口 #558/#559/#561/#562/#563 V/R/F | 依赖 AUDIT-001 Rust daemon 产物及堆叠 PR #542..#550/#558..#563，可执行 | PR #542..#550/#558..#563；§5.2、§6.2、§29..§37、§45..§51 | 主 agent；独立 V/R/F subagent |
| AUDIT-006 | Ready PR | 三个 backfill 业务能力、Rust Makefile产物和唯一 production backend image 发布路径已交付；migration operator lifecycle 已接入有界锁等待、信号退出、locked status 与恢复文档 | 异步收口 #555 PostgreSQL/entrypoint finding；不重复创建脱离 backend image 的第二套 backfill release assets | Rust image/package 入口可执行；真实生命周期交异步 V/R/F | PR #518/#519/#520/#523/#555；§6.2、§42 | 主 agent；独立 V/R/F subagent |
| AUDIT-007 | 待办 | feature-flag 等局部契约测试已有 | 把高风险 Go 回归按业务契约映射到 Rust 测试，不机械复制 807 个文件 | 可增量执行；最终索引依赖 AUDIT-002..006 能力矩阵稳定 | §6.2 | 主 agent；独立 V/R/F subagent |
| AUDIT-008 | 待办 | route parity 和部分 wire tests 已有 | 完成 JSON/时间/UUID-ULID/Redis/DB/event/旧数据兼容证据 | 可增量执行；最终兼容门依赖 AUDIT-002..006 的实际 wire 路径 | §6.2 | 主 agent；独立 V/R/F subagent |
| AUDIT-009 | 进行中 | 默认入口、pprof 和 logger 文档已有部分更新 | 对齐 install/systemd/release/rollback 及剩余运维文档 | 增量文档依赖对应实现；最终退出依赖 AUDIT-001..008 的真实路径 | PR #523/#524/#525；§6.2 | 主 agent；独立 V/R/F subagent |
| AUDIT-010 | 待办（最终门） | 尚无 Go 目录可删除 | 仅在 AUDIT-001..009 退出、生产验证通过后，做全仓引用审计并删除全部 Go 源文件 | 严格依赖 AUDIT-001..009 全部退出 | §6.2、§10 | 主 agent；独立 V/R/F subagent |

执行规则：一次只从“下一动作”选择一个不重叠的主线业务切片；切片完成后
立即提交、推送并创建 Ready PR，同时回写本表。verification/review/fix 可以
并行运行；主 agent 只从依赖已满足的项继续选择，不需要等待异步结果。切片大小
按完整生产能力/完整契约及其退出条件决定，不按行数决定；禁止仅为补测试、allow、
说明或制造小 PR 而拆开同一能力。

### 6.2 任务范围与退出证据

以下条目是执行台账的详细定义；它们规定每个任务何时真正完成。

#### P0

##### AUDIT-001：Rust 默认生产入口切换

- 范围：Makefile 的 server/cordy/build/migrate/test/dev/check、scripts/check.sh、scripts/dev.sh、Dockerfile、docker/entrypoint.sh、Helm backend、systemd/install/release 入口。
- 现状证据：默认后端和容器入口已由 PR #523 切到 Rust；PR #527 将 CLI release 资产链改为 Rust；PR #528 将 Desktop packaging 从 Go 源码嵌入改为按目标构建 Rust CLI；本切片对齐 installer/手工运维文档。
- 交付：默认产出 cordy-server、cordy-cli、cordy-migrate 及三个 Rust backfill；Rust release、Desktop 和 installer 保留兼容的 binary/asset 名称或有明确迁移说明；启动、迁移、信号、退出码和回滚路径可演练。
- 退出证据：新鲜 worktree 的 build/check、镜像启动 health/ready、migrate up/down/status、CLI --help/version、回滚演练均以 Rust 产物为准。
- owner：主 agent 迁移/接线；Volta 异步 review/fix。

##### AUDIT-002：生产行为与完整契约 smoke

- 范围：route parity 之外的认证、权限、事务、错误码/JSON、WS、realtime、background worker、CLI 退出码、daemon control/health。
- 交付：按业务能力建立可执行矩阵；每项标记 Go contract、Rust entry、生产是否切换、Go 是否可删。
- 退出证据：关键 API/WS/CLI/daemon smoke 在 Rust 默认产物上通过，并有失败路径和回滚记录。
- owner：主 agent 负责迁移与机械验证；review 与 fix 交给两个独立 subagent。

##### AUDIT-003：未闭合 leaf contract（pprof、logger、avatar、concurrency）

- pprof：Rust `cordy-server::profiling` 已在 127.0.0.1:6060 启动独立 listener，迁移 CPU profile、index、cmdline 和 symbol；heap/trace 尚未等价，必须继续迁移或明确替代并保持运维文档诚实。
- logger：Go 的 LOG_LEVEL、TTY color、component、request_id/user_id/client metadata 已在 Rust 入口对账；Rust 保留 RUST_LOG 作为未设置 LOG_LEVEL 时的兼容回退，默认级别与 Go 一样是 debug。
- Squad avatar：Rust `cordy-handler::squad` 已把响应接到现有 `avatar::resolve_url`，创建/更新接到 `avatar::accept_url`；这复用了已有 HMAC、存储归属和 standalone-image 发布校验，不重复实现 signer。私有对象的 squad 读写契约已迁移并接线；avatar endpoint 的下载策略与剩余 Go 退休仍需整体生产验证。
- agentconfig：Go 默认 max concurrent tasks 为 6、合法范围 1..50；Rust 由 `cordy-config::agent_concurrency` 统一 CLI/API contract。daemon 的默认 20 是独立的进程级 task slot pool，不是 agent 默认值；它从 `CORDY_DAEMON_MAX_CONCURRENT_TASKS`/CLI override 进入 `cordy-daemon::task_execution`，要求大于 0。
- 退出证据：每个 leaf 明确为“Rust 迁移并接线”“已由现有模块吸收”或“仍需迁移”，并有对应测试/生产路径。
- owner：主 agent 负责真正迁移；Volta 负责 review/fix。

##### AUDIT-004：integrations 生产配置矩阵

- provider：Lark、WeCom、DingTalk、Slack、Telegram、Composio、VCS、GitHub snapshot，以及 channel-engine/lease/media。
- 正向场景：有效凭证、真实 outbound、inbound/session 路由、media、重试和 shutdown。
- 负向场景：缺凭证、坏凭证、绑定缺失、网络失败必须可观测且 fail-closed；测试 Stub/Noop 不能被有效生产配置误选。
- 退出证据：每个 provider 有 Rust entry、配置开关、最小 smoke 或明确的不可测原因和回滚策略。
- owner：主 agent 负责迁移/生产接线；Volta 异步处理安全和回归修复。

##### AUDIT-005：daemon 完整能力验收

- 范围：control/health、registration、reconcile、runtime registry、provider refresh、task execution、wakeup/WS RPC、GC、repo cache、local skills、auto update、MCP broker。
- 现状：Rust production stack 已存在，但有 43 条 S9-integration 标记、28 个相关文件，且 crate 顶层仍写着“awaiting daemon wiring”。
- 交付：按真实调用关系逐项验收并移除已无意义的 seam/allow；若某 seam 是真实依赖，补真实 trait/entry，不做仅为清注释的 PR。
- 退出证据：daemon 生产进程可启动、控制面可用、task/provider/GC/reconcile 生命周期通过；不再依赖 Go daemon。
- owner：主 agent 迁移/接线；Volta review/fix。

##### AUDIT-006：migration 与 backfill 发布闭环

- Rust 已有 cordy-migrate 和 backfill_task_usage_hourly、backfill_issue_last_activity、backfill_codex_usage_cache 三个 bin；对应业务切片已在 PR #518、#519、#520。
- 当前 Dockerfile 只构建/复制两个旧 backfill，Makefile build 没有三个 Rust backfill 的默认产物，CI 仍以 Go migrate 为主验证之一。
- 交付：迁移 hooks、advisory lock、取消/超时、状态/退出码、三个 backfill 的 image/Makefile/release packaging 一致。
- 退出证据：新镜像只需 Rust migration/backfill 产物即可完成升级和运维恢复。
- owner：主 agent；Volta 异步 review/fix。

#### P1

##### AUDIT-007：Go 测试契约映射

- 不按 807 个 Go test 文件机械复制。
- 先按 API、DB transaction、provider、daemon lifecycle、security boundary、backfill、CLI contract 建索引。
- 每个高风险 Go 回归用例标记 Rust 已有测试、需新增测试、或不适用及理由。
- 退出证据：关键 contract 有 Rust 可执行测试；测试失败由 Volta 处理，主 agent 不代做修复。

##### AUDIT-008：wire/schema/ID 兼容性

- 对齐 JSON null/empty、时间格式、UUID/ULID、Redis key/channel、DB nullable/enum、错误码和事件 envelope。
- cordy-util 当前明确留下 ULID TODO：wrapper 的 serde 仍输出 UUID hyphenated string，而 Go wire contract 使用 26 字符 Crockford ULID；必须在删除 Go 前完成或证明所有当前路径不使用该 wrapper。
- 退出证据：golden vectors/round-trip/旧数据读取和跨语言事件 fixture 通过。

##### AUDIT-009：运维与文档切换

- 更新 SELF_HOSTING_ADVANCED.md、Helm 注释、README/install/systemd、release 说明、pprof/metrics/rollback 文档。
- 文档中的 go run ./cmd/...、go tool pprof 和 binary 名称必须与实际 Rust 产物一致。
- 只在 AUDIT-001 的默认入口确定后落地，避免先写一套与产物不一致的文档。

##### AUDIT-010：Go 源码退休门槛

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

## 11. AUDIT-001 执行更新

后续切片 `codex/cord-188-rust-production-cutover` 已开始收口 AUDIT-001 的后端生产边界：

- Rust 入口：`Makefile` 的 `server`、`cli`/`cordy`、`build`、`test`、`migrate-up/down`，以及 `scripts/dev.sh`、`scripts/check.sh`；统一通过 `server-rs` 的 `cordy-server`、`cordy`、`cordy-migrate`。
- 容器入口：`Dockerfile` 不再构建 Go runtime binary，改为构建 Rust server、CLI、migration runner 和三个 Rust backfill，并继续提供 `server`/`cordy`/`migrate` 兼容产物名；`docker/entrypoint.sh` 无需改名即可继续执行迁移后启动。
- CI/Helm：部署构建与迁移验证改用 Rust，并新增生产镜像构建门；Helm 的 backend 注释改为 Rust 入口事实。
- 生产路径状态：PR #523 覆盖本地默认入口、自托管镜像和 CI 部署验证；PR #527 将 CLI release workflow 和 Homebrew formula 输入改为 Rust；本切片将 Desktop 内嵌 CLI 的 smoke/release 构建改为 Rust。install/systemd 全链路和回滚目标仍未整体闭合，故 AUDIT-001 尚未完成。
- Go 是否可下线：否。Go compatibility build/test、CLI release/install、回滚目标和剩余 leaf contract 仍在清单中。
- 验证状态：shell 语法、`git diff --check`、Makefile entrypoint/build contract 已通过；Helm 未执行（审计环境无 `helm`），Docker 构建和 Rust workspace 编译继续按本切片记录，不以环境缺失冒充通过。

## 12. AUDIT-003 执行更新：Rust CPU pprof

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

## 13. AUDIT-003 执行更新：Rust logger contract

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

## 14. AUDIT-003 执行更新：Rust squad avatar contract

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

## 15. AUDIT-001 执行更新：Rust CLI release assets

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

## 16. AUDIT-001 执行更新：Desktop 内嵌 Rust CLI

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

## 17. AUDIT-001 执行更新：安装与运维入口对齐

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

## 18. AUDIT-002 执行更新：CLI 与 daemon control smoke 矩阵

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

## 19. AUDIT-003D 执行更新：agent 与 daemon concurrency contract

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

## 20. AUDIT-004 执行更新：Lark production configuration contract

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

## 21. AUDIT-004 执行更新：WeCom production configuration contract

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

## 22. AUDIT-004 执行更新：DingTalk production configuration contract

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

## 23. AUDIT-004 执行更新：Slack production configuration contract

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

## 24. AUDIT-004 执行更新：Telegram production configuration contract

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

## 25. AUDIT-004 执行更新：Composio production configuration contract

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

## 26. AUDIT-004 执行更新：VCS production configuration contract

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

## 27. AUDIT-004 执行更新：GitHub snapshot production configuration contract

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

## 28. AUDIT-004 执行更新：channel media production lifecycle contract

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

## 29. AUDIT-005 执行缺口：daemon health uptime wire contract

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

## 30. AUDIT-005 执行缺口：provider refresh partial-failure retry

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

## 31. AUDIT-005 执行缺口：GC metadata single wire contract

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

## 32. AUDIT-005 执行缺口：runtime MCP production merge

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

## 33. AUDIT-005 执行缺口：Remote MCP broker production wiring

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

## 34. AUDIT-005 执行缺口：plugin-hook MCP production wiring

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

## 35. AUDIT-005 执行缺口：local-skills heartbeat list/import/report contract

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

## 36. AUDIT-005 执行缺口：wakeup WebSocket / RPC / control consumer lifecycle

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

## 37. AUDIT-005 执行缺口：auto-update / server update / restart handoff contract

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

## 38. AUDIT-001 执行缺口：tag release verification Go dependency cutover

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

## 39. AUDIT-001 执行缺口：self-host Rust image upgrade/rollback ref ownership

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

## 40. AUDIT-001 执行缺口：self-host Rust Compose systemd lifecycle

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

## 41. AUDIT-001 执行缺口：required backend CI Go gate cutover

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

## 42. AUDIT-006 执行缺口：Rust migration operator lifecycle

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

## 43. AUDIT-003A 执行缺口：Rust process profiling replacement contract

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

## 44. AUDIT-003B 执行缺口：Rust operator log time layout

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

## 45. AUDIT-005 执行缺口：poisoned session retry and retirement lifecycle

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

## 46. AUDIT-005 执行缺口：Codex session rollout durability and continuity

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

## 47. AUDIT-003A 执行缺口：Rust allocation heap profile and async runtime diagnostics

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

## 48. AUDIT-005 执行缺口：confirmed provider demotion and recovery lifecycle

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

## 49. AUDIT-005 执行缺口：private socket-safe task temp lifecycle

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

## 50. AUDIT-005 执行缺口：wakeup environment proxy and CONNECT lifecycle

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

## 51. AUDIT-005 执行缺口：heartbeat stale HTTP pool recovery lifecycle

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
  触发 swap，success、明确 `runtime not found` 的 404 与 WS freshness 重置 streak；普通永久 4xx 和 parent
  cancellation 不累计 failure。后续 heartbeat 可恢复；auth/identity headers 与非 HTTP client state 由静态
  centralized builder/唯一 production assembly 核对，不冒充本 socket test 的动态覆盖。
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

## 52. AUDIT-002 执行缺口：issue-status production API and transaction contract

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

## 53. AUDIT-002 执行缺口：issue create admission and column ordering transaction contract

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

独立 verifier 在 exact HEAD `1bf444767ebef7103403c563a3112d5a7b4eef9d` 完成首轮：clean/range diff、base
ancestry 和 Cargo.lock unchanged PASS；fixed stable rustfmt 对本 PR 新 handler/service test code exit 1。locked/offline
metadata、两个 crate no-run、server/Windows checks 与四个 exact tests 都被继承 #563 `hyper-util runtime` feature
resolver error 阻断并 exit 101；四个 filter matched/executed/ignored 均 `0/0/0`，不能记为通过。环境无
`DATABASE_URL`/`psql`/`pg_isready`，真实 concurrency/position/HTTP/autopilot/cleanup 全未执行；required-DB tests 本身
不会 self-skip，但 harness 未启动。direct rustfmt 与 inherited resolver 已交独立 fixer，原始失败保留。

独立 reviewer 锚定同一 HEAD：无 P0，2 个 P1、4 个 P2。P1：Rust HTTP duplicate 409 使用泛化 error，而 Go 使用
已有 `duplicate_message` 的 identifier/title/status 完整 wire，新 test 锁定了错误值；recent autopilot 只直调 helper，
未经过真实 dispatch/position caller。P2：advisory concurrency 用固定 sleep 推测到达点且未证明 allow_duplicate 仍
加锁；identity/effective/autopilot matrix 声明宽于断言；HTTP test 注入 WorkspaceContext 绕过 production auth/
workspace middleware；fixture 只在 happy path cleanup，panic/timeout 会污染共享 DB。reviewer 确认 required-DB fail
方向、唯一普通/autopilot production caller、无 Stub/Noop/Fake/alternate allocator，且两既有文件/无新依赖方向合理，
但约 500 行证据仍不足以支持完整下线。全部 finding 已交独立 fixer，PR 保持 Ready。
independent fixer 传播 #560..#564 已验证的 Cargo/resolver 修复后，按最小根因收口本切片：

- `issue_status::resolve` 现在以 typed error 区分 unknown/archived 与 storage failure；真实 issue create、single update、
  transactional recheck 与 batch preflight 只把前者映射为既有 4xx，后者记录并返回 500，不再把 DB 故障伪装成
  invalid status。新增无 DB server 依赖的 directed test 直接证明 storage error 保留。
- DB contract fixture 缺少或无法连接 `DATABASE_URL` 时现在明确 panic；不得再以成功 self-return 冒充真实 DB
  contract PASS。TestFlags 直接断言 production key `custom_issue_statuses` 与 fail-closed default。update statement 因
  concurrent archive/guard 返回空行时按 Go wire contract 返回 409 `status is no longer editable`。
- create-wins race 不再由 test 自己持有 shared advisory lock：真实 `IssueService::create` 先在 production path 获取
  shared lock，再被 workspace row lock 确定性暂停；独立连接以 `pg_try_advisory_lock` 观察该真实 acquisition 后才
  启动 archive。batch 仍只有 production route happy-path、reorder 仍未补全 reviewer 列出的 malformed/foreign/
  built-in/archived/cross-category wire matrix，因此台账不再声称这些未执行项已具备直接证据，Go 下线门未满足。
- fixer verification：historical rustfmt exit 1、resolver exit 101/0 tests 与 DB case 未执行记录保留；传播后
  `cargo check --locked --offline -p cordy-handler -p cordy-service --tests` PASS；exact write-policy 1/1 PASS
  （385 filtered）、exact storage-error 1/1 PASS（168 filtered）、color exact 1/1 PASS（385 filtered）；fixed-stable
  rustfmt 与 `git diff --check` PASS。当前环境无 `DATABASE_URL`，exact production catalog test 确实运行 1 项并
  FAIL（0 passed，385 filtered，明确 `DATABASE_URL is required`），所以新的 real-DB race/reorder/catalog 行为没有
  被本 fixer 环境验证，不能记为通过或支持 Go 退休。
- independent fixer 将 #560..#563 的 lock/resolver 修复依次传播到本分支，并最小复用现有
  `client::is_transient_error`：仅 transport/5xx/408/429 累计 streak；永久 4xx 重置，root cancellation
  立即退出；新增 anyhow error-chain predicate 只在 404 body 含 `runtime not found` 时发 `RuntimeGone`。
  production heartbeat socket test 现以确定序列直接覆盖 500、401、普通 404、runtime-not-found 404、408、
  pool retirement 和 cancellation；原有 pool lifetime test 继续证明 in-flight request 不被 swap 取消。
- fixer verification：historical exact HEAD 的 rustfmt exit 1 与 resolver exit 101/0 tests 保留；传播后
  `cargo check --locked --offline -p cordy-daemon --tests` PASS；exact production classification test 1/1 PASS
  （472 filtered）；exact in-flight pool lifetime test 1/1 PASS（472 filtered）；`manager::tests::` 10/10 PASS
  （463 filtered）；fixed-stable `client.rs`/`manager.rs` rustfmt 与 `git diff --check` PASS。loopback tests 需
  sandbox 外 bind 权限；其余输出只有堆叠基线 warnings。尚未执行 Windows build 或外部 NAT/LB smoke，
  因此 Go 退休仍等待整个 AUDIT-005 门，而非由本切片单独宣称完成。
  `codex/cord-226-private-task-temp-rust` at `1bf5cccd`。PR 为非 Draft Ready；verification/reviewer 待异步
  派发，fixer 尚无本 PR finding。
- fixer（基于 review ledger HEAD `9c4227c4`）：Cargo 正常解析并提交完整新增依赖图；复用已批准的
  #536 SecretBox fixture 与 #544 `GCMetaKind` 修复解除两个堆叠编译阻断。Tokio console 现在仅在
  `CORDY_TOKIO_CONSOLE=1` 时启用，固定 loopback socket 在继续启动前完成 bind，冲突 fail-closed；
  `ConsoleLayer::build` 返回的 server 与现有 profiling cancellation/runtime 一起显式 serve、cancel、join，
  retention 固定 60 秒、event buffer 固定 1024，Compose 显式传递开关。heap capture 的 profiler lock、
  tempfile、dump、parse、gzip 全部在 `spawn_blocking` 内执行；runbook 解析 console client 绝对路径并验证
  backend PID。
- fixer verification：`cargo metadata --locked --offline --format-version 1 --no-deps` PASS；
  `CARGO_INCREMENTAL=0 cargo check --locked --offline -p cordy-server --bin cordy-server` PASS；
  `CARGO_INCREMENTAL=0 cargo check --locked --offline -p cordy-server --tests` PASS；从 `server-rs`（确保读取
  production `.cargo/config.toml` 的 `tokio_unstable`）以
  `CARGO_INCREMENTAL=0 cargo test --locked --offline -p cordy-server --bin cordy-server
  'profiling::tests::' -- --nocapture` 运行 8/8 PASS（含 opt-in、occupied bind、cancel join、真实 heap gzip
  与 pprof protobuf decode、private router/legacy trace）。首次 test no-run 在共享磁盘仅余 2.8 MiB 时
  ENOSPC，清理本 worktree target 后 no-run PASS；首次 loopback test 在 sandbox bind 处失败，非沙箱重跑
  PASS；从仓库外层运行 console test 因未读取嵌套 Cargo config 而正确拒绝缺少 `tokio_unstable` 的产物，
  从 `server-rs` 重建后 PASS。fixed-stable main/profiling rustfmt、`git diff --check` PASS。
- 未执行：external `tokio-console` client task/resource/operation 观察、release/Docker/non-Linux/musl build、
  public production router live 404、SIGTERM production process smoke 与负载 CPU/RSS overhead。默认聚合已关闭
  并给出容量上界，但这些 smoke/开销项仍不得记为通过，AUDIT-003A 保持部分完成。
- #561 independent fixer（基于 review ledger `93d38c16`，上游依赖传播 commit `fdbaef0b`）：统一 registration
  publication 的 barrier→workspace serial 锁序；busy demotion 改为原子 try-barrier 后 defer，不再等待期间冻结
  新 claim；workspace deregister 失败记录 pending 并在后续 round best-effort 重试且不中断其余 workspace。
  server 接受的 runtime 尚未进入 authoritative registry 前先在同一 admission critical section 发布对应 launch
  spec，消除 registry 可见而 launch 尚不可解的 claim 窗口。`LocalProviderCatalog` 以启动时 accepted `AgentEntry`
  与 fresh discovery 合并，fresh path 优先、discovery miss 保留为 unavailable/self-heal 候选；probe 使用
  `buffer_unordered(8)`，空 version 复用 last-known。production health 直接读取 registration source 的最新
  unavailable/demotable reason，填充 `skipped_agents`。
- #561 fixer verification：继承 blocker 修复后
  `CARGO_INCREMENTAL=0 cargo check --locked --offline -p cordy-daemon --tests -p cordy-cli` PASS；原 verifier
  指定的 provider/registry/registration 7 条 exact tests 全部 1/1 PASS；新增
  `accepted_provider_survives_discovery_miss_and_fresh_path_wins` 与
  `nonblocking_claim_barrier_defers_without_pausing_busy_daemon` 各 1/1 PASS；registration/provider registration
  聚焦组 14/14 PASS。fixed-stable 六个相关文件 rustfmt check 与 `git diff --check` PASS。历史保留：reviewed
  head 的 metadata/no-run/exact tests 均因继承 #560 lock inconsistency 以 101/0-test 失败，三文件 rustfmt FAIL；
  blocker 传播后已进入真实编译/执行。
- #561 尚未执行真实 server loopback deregister failure→next-round retry、完整 CLI daemon foreground smoke 与
  多 workspace 高延迟 probe wall-clock smoke；因此这些外部/时序证据不记为 PASS，Ready 声明仅覆盖上述已执行
  单元与 compile contract。
- #562 independent fixer（基于 review ledger `1bf5cccd`，依赖传播 commits `50af728a`/`83468e0b`）：
  non-Windows `CORDY_AGENT_TEMP_BASE` 的 UTF-8 value 先 `trim`，空白视为 unset，带空格 absolute value 使用
  trim 后路径；non-Unicode value 在 allocation 前以含变量名的明确错误 fail-closed，删除与 production
  `to_str` 冲突的 false-accept contract。三个重复 test-local restore 实现合并为一个 guard，并由 execenv 与
  adapter test 共享同一环境锁。新增唯一 `ProductionProviderAdapter::run_task_inner` Kiro loopback 测试：
  server start handler 先写 sentinel，真实 child 才能启动；child 捕获并证明 `TMPDIR`/`TMP`/`TEMP` 三值一致且
  覆盖恶意 custom env；success 与 server start failure 两条退出路径均断言 configured temp base 无残留。
- #562 fixer verification：`cargo metadata --locked --offline --format-version 1 --no-deps` PASS；
  `CARGO_INCREMENTAL=0 cargo check --locked --offline -p cordy-daemon --tests` PASS；task-temp allocation 4/4
  PASS，execution-plan authoritative rebind exact 1/1 PASS，production adapter lifecycle exact 1/1 PASS；后者
  sandbox 首次因 loopback bind `EPERM` 0/1 FAIL，非沙箱重跑后 PASS。首次 direct test 迭代还分别暴露并修正
  missing agent id/name/identity、非 `mat_` token 与 `/tmp` noexec fixture（改由 `/bin/sh` 执行），最终使用真实
  production validation/backend path PASS。fixed-stable 三个 #562 文件 rustfmt 与 `git diff --check` PASS。
- #562 未执行 Windows runtime smoke、process cancellation/forced-kill cleanup 或完整 daemon foreground smoke；
  direct adapter 证据覆盖 success 与 StartTask failure，但不把未执行的 build/launch/cancel/failure 全矩阵记为
  PASS。
- #563 independent verifier/reviewer 历史（exact `2180ded88a00f48fe31e60e09d963c6d1c936a6a`）：
  fixed-stable `manager.rs` rustfmt FAIL；workspace `hyper-util` 请求 0.1.20 不存在的 `runtime` feature，导致
  metadata、daemon no-run、三个新增 exact、existing real WebSocket 与 Windows check 全部 101，实际 0 tests。
  reviewer 另确认 lowercase/uppercase/CGI precedence、NO_PROXY port/IPv6/leading-dot/wildcard、IPv6 proxy dial、
  cancellation、strict CONNECT status 与 userinfo 存在 parity 缺口；原 claims 超过实际执行证据。
- #563 independent fixer（依赖传播 commits `3e2e88ca`/`6a3c6b57`/`741d9857`）：删除 daemon 对
  `client-proxy` matcher 的依赖，并从 workspace hyper-util features 移除不存在的 `runtime`，由 Cargo 离线
  正常更新 lock。唯一 manager dial 使用小型 `WakeupProxy`：lowercase proxy/no_proxy 优先；CGI 仅拒绝被选中
  的 uppercase `HTTP_PROXY`；NO_PROXY 直接覆盖 exact/apex+subdomain/leading-dot/`*.`/port/IPv4+IPv6/CIDR；
  IPv6 proxy address 统一 bracket formatting。proxy userinfo 严格 percent decode/UTF-8，username-only 生成
  Basic `user:`；CONNECT 只接受合法 HTTP/1.0/1.1 三位 status line。整个 DNS/TCP/CONNECT/TLS/WS future 同时
  受 parent context 与既有 handshake timeout 约束。
- #563 fixer verification：locked/offline metadata 与
  `CARGO_INCREMENTAL=0 cargo check --locked --offline -p cordy-daemon --tests` PASS；selection/NO_PROXY/auth/IPv6、
  env precedence+CGI、parent cancellation、CONNECT refusal/truncation/malformed/oversize、production child exact
  CONNECT+Basic+target TLS、existing real WebSocket 六条 exact tests 各 1/1 PASS。fixed-stable manager rustfmt 与
  `git diff --check` PASS；manager 聚焦组 9/9 PASS。真实 CONNECT/WebSocket tests 在非沙箱 loopback 权限下执行；reviewed head 的 resolver
  101/0-test 与 rustfmt failure 均保留为历史。
- #563 尚未执行真实 corporate proxy/DNS failure、Windows target compile 或 HTTP polling live fallback smoke；
  `cordy-lark` 仍有独立 CONNECT 实现，跨 crate 共享 primitive 作为 P3 debt 保留，本切片不借机扩大公共网络
  抽象，也不把这些未执行项声明为通过。
