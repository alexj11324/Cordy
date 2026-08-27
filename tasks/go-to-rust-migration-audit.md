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
| AUDIT-001 | 进行中 | 默认 server、CLI、migration、Docker、CI、Helm、CLI release 资产链和 Desktop 内嵌 CLI 已切到 Rust | 收口 install/systemd、兼容产物、启动与回滚演练 | 入口切换已可交付；最终生产验收依赖 AUDIT-002..009 退出 | PR #523/#527/当前切片；详见 §11、§15、§16 | 主 agent；独立 V/R/F subagent |
| AUDIT-002 | 进行中 | 已有 route parity、局部包测试；当前切片建立 CLI 命令树/退出码/daemon control smoke 矩阵 | 先收口 CLI/daemon 矩阵，再补 API/WS/事务/错误 JSON 和 background worker 的真实 smoke | 依赖 AUDIT-001 已交付的 Rust 默认产物；各域 smoke 随 AUDIT-003..006 落地 | §5、§6.2、§18 | 主 agent；独立 V/R/F subagent |
| AUDIT-003A | 部分完成 | CPU/cmdline/symbol pprof 已接入 Rust | heap/trace 等 Go profiling 能力完成等价迁移，或形成明确替代与运维证据 | Rust server 入口已由 AUDIT-001 交付，可执行 | PR #524；详见 §12 | 主 agent；独立 V/R/F subagent |
| AUDIT-003B | 部分完成 | logger 配置、TTY、component、request attrs 已接入 Rust | 决定并验证剩余时间布局兼容性，不扩大为新日志框架 | Rust server/daemon 入口已由 AUDIT-001 交付，可执行 | PR #525；详见 §13 | 主 agent；独立 V/R/F subagent |
| AUDIT-003C | Ready PR | squad avatar 读写已接入既有 avatar capability | 等待异步 V/R/F，并纳入生产对象存储 smoke | 依赖 AUDIT-004 的生产存储证据完成退出 | PR #526；详见 §14 | 主 agent；独立 V/R/F subagent |
| AUDIT-003D | Ready PR | agent 的每实体限额已集中为默认 6、范围 1..50；daemon 的进程级 slot pool 独立保持默认 20、要求 >0 | 等待异步 V/R/F；生产 daemon 生命周期 smoke 继续归 AUDIT-005 | 配置契约可执行；最终退出依赖 AUDIT-005 daemon 生命周期 | PR #531；§6.2、§19 | 主 agent；独立 V/R/F subagent |
| AUDIT-004 | 主线切片已交付 | Lark、WeCom、DingTalk、Slack、Telegram、Composio、VCS、GHSnapshot 与 channel media production lifecycle 已交付 | verification 收口 supervisor/lease 矩阵、外部凭证 smoke/不可测原因与回滚策略；review/fix 异步回写 | 主 agent 当前无新的不重叠迁移缺口；最终退出依赖异步 V/R/F 直接证据 | PR #532..#536/#538..#541；§5.3、§6.2、§20..§28 | 主 agent；独立 V/R/F subagent |
| AUDIT-005 | 进行中 | `/health` uptime、provider refresh 重试、GC metadata 单一 wire contract 和 runtime MCP production merge 已交付；当前选定 Remote MCP broker production wiring | 启动 task-local secure broker、合并 overlay 并绑定 task lifetime，再继续 plugin-hook MCP | 依赖 AUDIT-001 已交付的 Rust CLI/daemon 产物及 #545 effective config 路径，可执行 | PR #542..#545/当前切片；§5.2、§6.2、§29..§33 | 主 agent；独立 V/R/F subagent |
| AUDIT-006 | 进行中 | 三个 backfill 业务能力已由 PR #518/#519/#520 交付，默认镜像入口已开始切 Rust | 收口 migration/backfill 的 Makefile、image、release、锁、取消和恢复证据 | 依赖 AUDIT-001 已交付的 Rust image/package 入口，可执行 | PR #518/#519/#520/#523；§6.2 | 主 agent；独立 V/R/F subagent |
| AUDIT-007 | 待办 | feature-flag 等局部契约测试已有 | 把高风险 Go 回归按业务契约映射到 Rust 测试，不机械复制 807 个文件 | 可增量执行；最终索引依赖 AUDIT-002..006 能力矩阵稳定 | §6.2 | 主 agent；独立 V/R/F subagent |
| AUDIT-008 | 待办 | route parity 和部分 wire tests 已有 | 完成 JSON/时间/UUID-ULID/Redis/DB/event/旧数据兼容证据 | 可增量执行；最终兼容门依赖 AUDIT-002..006 的实际 wire 路径 | §6.2 | 主 agent；独立 V/R/F subagent |
| AUDIT-009 | 进行中 | 默认入口、pprof 和 logger 文档已有部分更新 | 对齐 install/systemd/release/rollback 及剩余运维文档 | 增量文档依赖对应实现；最终退出依赖 AUDIT-001..008 的真实路径 | PR #523/#524/#525；§6.2 | 主 agent；独立 V/R/F subagent |
| AUDIT-010 | 待办（最终门） | 尚无 Go 目录可删除 | 仅在 AUDIT-001..009 退出、生产验证通过后，做全仓引用审计并删除全部 Go 源文件 | 严格依赖 AUDIT-001..009 全部退出 | §6.2、§10 | 主 agent；独立 V/R/F subagent |

执行规则：一次只从“下一动作”选择一个不重叠的主线业务切片；切片完成后
立即提交、推送并创建 Ready PR，同时回写本表。verification/review/fix 可以
并行运行；主 agent 只从依赖已满足的项继续选择，不需要等待异步结果。

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
- 异步状态：verification 与 reviewer 已派发但尚未返回；fixer 尚未派发。主 agent
  只确认 `git diff --check` 无错误，不把它记录成编译、测试或生产验证通过。PR 堆叠
  在 Ready PR #541；上游 #538 已记录的 Tokio-context 测试失败仍由其独立 fixer处理。

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
- 异步状态：verification 与 reviewer 已派发但尚未返回；fixer 尚未派发。主 agent
  只确认 `git diff --check` 无错误，不记录为编译或测试通过。PR 堆叠在 #542。

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

本切片只接通 broker startup、credential resolver、optional diagnostics、fatal failure、
config merge 和 task lifetime ownership。实现、Ready PR、verification、review 和 fix
尚未产生，不能记录通过。
