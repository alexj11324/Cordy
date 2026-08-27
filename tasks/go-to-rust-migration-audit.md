# Go→Rust 全量迁移：独立基线审计与执行 TODO

> 审计快照：2026-08-27 UTC
> 审计基线：0f92fb042ffc742b8dcf8af91cea3d97716c05a4
> 审计分支：codex/cord-187-go-rust-migration-audit-v2
> 范围：server/、server-rs/、默认运行/构建/发布/部署链路

这是一份独立的全局基线，不是按“完成一块再查一块”生成的局部记录。后续迁移只能从本清单选择切片；完成切片后更新对应证据和状态，再选择下一块。它取代 tasks/go-to-rust-migration.md 中互相矛盾的当前状态判断；旧文件保留为历史执行记录。

## 1. 执行边界

主 agent 只做：

- Go→Rust 业务能力或完整契约迁移；
- Rust 生产入口接线；
- worktree、分支、提交、推送和 Ready PR；
- 机械性编译、测试、契约检查和结果记录。

Volta subagent 异步负责：

- review；
- 缺陷、安全问题、测试失败和回归修复；
- 必要时直接提交修复。

review/fix 派发后不等待、不轮询，也不把其结果作为下一块迁移或 Ready PR 的前置条件。主 agent 不在主线自行处理 review 意见或修复任务。

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
| local/S3/CloudFront storage | handler::attachment_storage、cloudfront、attachment | cordy-server main 注入 attachment storage | 否 | 主存储能力已落地/接线；Squad avatar 返回仍有 signer 缺口，归 AUDIT-003 |
| CLI bins（cordy、migrate、3 backfill） | cordy-cli、cordy-migrate 及 3 个 Rust backfill bin | Rust bin 可独立运行 | 否，Makefile/Docker/release 仍产出 Go | Rust bin 已存在；构建产物、命令行为、安装/发布和 Docker packaging 未闭环 |
| pprof、logger | Go internal/profiling、internal/logger | Rust 只有 tracing 初始化证据 | 否 | Rust 未找到 Go pprof listener；LOG_LEVEL、TTY/color、RequestAttrs 等 logger 契约未证明等价 |
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
| internal/logger | cordy-server/cordy-daemon tracing 初始化 | 可能是吸收式迁移；LOG_LEVEL、属性和输出格式待证明 |
| internal/migrations | cordy-migrate runner/hooks | 已落地；默认入口仍 Go |
| internal/profiling | 未发现 Rust pprof 等价实现 | 明确未完成，见 AUDIT-003 |
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

### P0

#### AUDIT-001：Rust 默认生产入口切换

- 范围：Makefile 的 server/cordy/build/migrate/test/dev/check、scripts/check.sh、scripts/dev.sh、Dockerfile、docker/entrypoint.sh、Helm backend、systemd/install/release 入口。
- 现状证据：Makefile、Dockerfile、scripts 和 Helm 当前仍直接调用 server/cmd/server、server/cmd/cordy、server/cmd/migrate 的 Go 命令；Rust 只有独立 rust-cli/build-rust-cli 入口。
- 交付：默认产出 cordy-server、cordy-cli、cordy-migrate 及三个 Rust backfill；保留兼容的 binary/entrypoint 名称或有明确迁移说明；启动、迁移、信号、退出码和回滚路径可演练。
- 退出证据：新鲜 worktree 的 build/check、镜像启动 health/ready、migrate up/down/status、CLI --help/version、回滚演练均以 Rust 产物为准。
- owner：主 agent 迁移/接线；Volta 异步 review/fix。

#### AUDIT-002：生产行为与完整契约 smoke

- 范围：route parity 之外的认证、权限、事务、错误码/JSON、WS、realtime、background worker、CLI 退出码、daemon control/health。
- 交付：按业务能力建立可执行矩阵；每项标记 Go contract、Rust entry、生产是否切换、Go 是否可删。
- 退出证据：关键 API/WS/CLI/daemon smoke 在 Rust 默认产物上通过，并有失败路径和回滚记录。
- owner：主 agent 负责迁移与机械验证；缺陷交给 Volta。

#### AUDIT-003：未闭合 leaf contract（pprof、logger、avatar、concurrency）

- pprof：Go internal/profiling 在 127.0.0.1:6060 提供 /debug/pprof/；Rust server 未发现等价 listener。必须迁移或明确替代并更新运维文档。
- logger：Go 的 LOG_LEVEL、TTY color、component、request_id/user_id/client metadata 行为需要与 Rust tracing 对账；Rust 当前证据主要是 RUST_LOG/tracing subscriber。
- Squad avatar：Rust squad.rs 的 SquadResponse 直接返回 raw avatar_url，并注明 HandlerState 尚未携带 Go object-store signer；Go squadToResponse 会调用 resolveAvatarURLPtr。必须补完整的私有对象 URL/签名契约或证明当前存储策略等价。
- agentconfig：Go 默认 max concurrent tasks 为 6、合法范围 1..50；Rust handler 有 inline 1..50，但 daemon config 有独立默认值。必须确认这是两个不同边界还是迁移遗漏，形成单一 contract 证据。
- 退出证据：每个 leaf 明确为“Rust 迁移并接线”“已由现有模块吸收”或“仍需迁移”，并有对应测试/生产路径。
- owner：主 agent 负责真正迁移；Volta 负责 review/fix。

#### AUDIT-004：integrations 生产配置矩阵

- provider：Lark、WeCom、DingTalk、Slack、Telegram、Composio、VCS、GitHub snapshot，以及 channel-engine/lease/media。
- 正向场景：有效凭证、真实 outbound、inbound/session 路由、media、重试和 shutdown。
- 负向场景：缺凭证、坏凭证、绑定缺失、网络失败必须可观测且 fail-closed；测试 Stub/Noop 不能被有效生产配置误选。
- 退出证据：每个 provider 有 Rust entry、配置开关、最小 smoke 或明确的不可测原因和回滚策略。
- owner：主 agent 负责迁移/生产接线；Volta 异步处理安全和回归修复。

#### AUDIT-005：daemon 完整能力验收

- 范围：control/health、registration、reconcile、runtime registry、provider refresh、task execution、wakeup/WS RPC、GC、repo cache、local skills、auto update、MCP broker。
- 现状：Rust production stack 已存在，但有 43 条 S9-integration 标记、28 个相关文件，且 crate 顶层仍写着“awaiting daemon wiring”。
- 交付：按真实调用关系逐项验收并移除已无意义的 seam/allow；若某 seam 是真实依赖，补真实 trait/entry，不做仅为清注释的 PR。
- 退出证据：daemon 生产进程可启动、控制面可用、task/provider/GC/reconcile 生命周期通过；不再依赖 Go daemon。
- owner：主 agent 迁移/接线；Volta review/fix。

#### AUDIT-006：migration 与 backfill 发布闭环

- Rust 已有 cordy-migrate 和 backfill_task_usage_hourly、backfill_issue_last_activity、backfill_codex_usage_cache 三个 bin；对应业务切片已在 PR #518、#519、#520。
- 当前 Dockerfile 只构建/复制两个旧 backfill，Makefile build 没有三个 Rust backfill 的默认产物，CI 仍以 Go migrate 为主验证之一。
- 交付：迁移 hooks、advisory lock、取消/超时、状态/退出码、三个 backfill 的 image/Makefile/release packaging 一致。
- 退出证据：新镜像只需 Rust migration/backfill 产物即可完成升级和运维恢复。
- owner：主 agent；Volta 异步 review/fix。

### P1

#### AUDIT-007：Go 测试契约映射

- 不按 807 个 Go test 文件机械复制。
- 先按 API、DB transaction、provider、daemon lifecycle、security boundary、backfill、CLI contract 建索引。
- 每个高风险 Go 回归用例标记 Rust 已有测试、需新增测试、或不适用及理由。
- 退出证据：关键 contract 有 Rust 可执行测试；测试失败由 Volta 处理，主 agent 不代做修复。

#### AUDIT-008：wire/schema/ID 兼容性

- 对齐 JSON null/empty、时间格式、UUID/ULID、Redis key/channel、DB nullable/enum、错误码和事件 envelope。
- cordy-util 当前明确留下 ULID TODO：wrapper 的 serde 仍输出 UUID hyphenated string，而 Go wire contract 使用 26 字符 Crockford ULID；必须在删除 Go 前完成或证明所有当前路径不使用该 wrapper。
- 退出证据：golden vectors/round-trip/旧数据读取和跨语言事件 fixture 通过。

#### AUDIT-009：运维与文档切换

- 更新 SELF_HOSTING_ADVANCED.md、Helm 注释、README/install/systemd、release 说明、pprof/metrics/rollback 文档。
- 文档中的 go run ./cmd/...、go tool pprof 和 binary 名称必须与实际 Rust 产物一致。
- 只在 AUDIT-001 的默认入口确定后落地，避免先写一套与产物不一致的文档。

#### AUDIT-010：Go 源码退休门槛

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
3. 立即把 review/fix 派给 Volta，继续下一块，不等待；
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
