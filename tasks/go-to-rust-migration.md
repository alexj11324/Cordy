# Go → Rust 全量迁移计划（server/）

> 决策记录：战略技术栈决策 · 范围全量 server/ · 策略一次性替换（big-bang）
> 日期：2026-08-20 · 状态：执行中

## 一、Codemap（现状）

### 规模
| 指标 | 数值 |
|------|------|
| Go 源文件 | 1,444 |
| 非测试代码 | ~276,000 行 |
| 测试文件 | 807 |
| 内部包 | 37 |

### 模块规模排序（非测试行数）
```
handler/        66,287   HTTP API 层（111 文件，730 个 Handler 方法，扁平包）
daemon/         36,982   本地 agent 守护进程（配对/健康/GC/自更新/artifact）
integrations/   35,993   外部集成：channel/composio/dingtalk/ghsnapshot/lark/slack/telegram/vcs/wecom
service/        15,429   业务逻辑（autopilot/plugin(MCP)/email/cron/issue...）
metrics/         3,722   Prometheus 自定义指标
realtime/        2,943   WS Hub + Redis relay + 分片流 relay + retention
cli/             2,377   CLI 框架
scheduler/       1,720   cron 任务
middleware/      1,427   auth/角色守卫/CSP/CORS
auth/            1,343   JWT + PAT + CloudFront signer
其余             <1,200   analytics/attribution/entitlement/featureflags/storage/util...
```

### 入口点（cmd/）
- `cordy` — 主 CLI（cobra）
- `server` — HTTP 服务（main.go 装配 + router.go **2,352 行 / 475 条路由**）
- `migrate` — 迁移执行器
- `backfill_*` ×3 — 数据回填工具

### 关键架构事实
- **Handler 是上帝对象**：`New(queries, txStarter, hub, bus, emailService, store, cfSigner, analyticsClient, cfg, daemonHubs...)`
- **路由集中在** `cmd/server/router.go`，中间件链：RequestID → ClientMetadata → RequestLogger → HTTPMetrics → Recoverer → CSP → CORS → Auth 变体(DaemonAuth/PluginAuth/Auth) → CloudFront cookies → workspace 成员/角色守卫
- **双 WebSocket 面**：`realtime.Hub`（用户实时）+ `daemonws.Hub`（守护进程），Redis relay 支持多实例 fanout
- **数据层**：sqlc(pgx/v5) 54 个查询文件 → 生成代码；**826 个迁移文件（413 对 up/down）**
- **事件总线**：`events/bus.go` 进程内总线

## 二、测试评估与验证策略

### 现状
- 测试为**集成测试风格**：`make test` = ensure-postgres.sh → migrate up → `go test --race`
- `testutil/` 提供 DB Fixture（Insert/Cleanup/Count）+ HTTP 测试工具
- 无 mock 泛滥问题——直接打真库

### Rust 侧验证策略
| 层级 | 命令 | 说明 |
|------|------|------|
| 编译 | `cargo check --workspace` | 每步必跑 |
| 单元测试 | `cargo test --workspace` | 纯逻辑测试 |
| 集成测试 | `cargo test --workspace --features integration` | 复用同一 Postgres（testcontainer 或直连） |
| Lint | `cargo clippy --workspace -- -D warnings` | 提交前 |
| 格式 | `cargo fmt --check` | 提交前 |

### 兼容性硬约束（不可破坏）
1. **API 契约不变** — web/desktop/mobile 三端依赖
2. **WebSocket 消息格式不变** — realtime + daemonws 协议
3. **数据库 schema 不变** — Rust 直接复用 `server/migrations/` 同一套 SQL
4. **Redis key 格式不变**
5. **ULID/时间戳 JSON 序列化格式不变**

## 三、Rust 技术栈选型

| Go 依赖 | Rust 替代 | 备注 |
|---------|-----------|------|
| chi/v5 | **axum 0.8** | tower 中间件生态，WS 原生支持 |
| gorilla/websocket | axum::extract::ws | |
| pgx/v5 + sqlc | **sqlx 0.8**（query! 宏编译期校验） | 手写 SQL 迁移自 queries/，获得编译期校验 |
| go-redis | **redis-rs** (deadpool-redis 连接池) | |
| aws-sdk-go-v2 | **aws-sdk-rust**（官方） | S3 + Secrets Manager |
| openai-go | **async-openai** | 流式 SSE 支持 |
| slack-go | **slack-morphism** | Socket Mode + Events API |
| resend-go | reqwest 自封装 | Resend REST API 极简 |
| golang-jwt | **jsonwebtoken** | |
| cobra | **clap 4** (derive) | |
| robfig/cron | **tokio-cron-scheduler** | |
| prometheus/client | **prometheus** crate（官方） | |
| go-toml | **toml** + serde | |
| tint/slog | **tracing** + tracing-subscriber | 结构化日志标准 |
| oklog/ulid | **ulid** crate | serde 序列化需对齐大写字符串格式 |
| resty | **reqwest** | |
| mattn/go-shellwords | shell-words | |

### Workspace 结构
```
server-rs/
├── Cargo.toml              # workspace
├── crates/
│   ├── cordy-config/       # 配置加载（TOML+env）
│   ├── cordy-util/         # ULID/时间/错误类型
│   ├── cordy-db/           # sqlx pool + queries + migrate runner
│   ├── cordy-auth/         # JWT/PAT/CloudFront signer
│   ├── cordy-realtime/     # WS hub + redis relay
│   ├── cordy-events/       # 进程内事件总线
│   ├── cordy-service/      # 业务服务层
│   ├── cordy-integrations/ # slack/telegram/lark/wecom/dingtalk/vcs...
│   ├── cordy-daemon/       # 守护进程协议
│   ├── cordy-handler/      # HTTP handlers + router
│   └── bins: cordy-server / cordy-migrate / cordy-cli
```

## 四、执行顺序（依赖拓扑）

- [x] **S1. 脚手架**：workspace + config/util/db 三 crate + CI lint 配置
- [x] **S2a. 本地 Postgres**：brew postgresql@17 装机 + cordy_rs 测试库
- [x] **S2b. 迁移器**：cordy-migrate 完整移植（advisory lock 7244554146635925501、schema_migrations 表、124+12 并发索引清理钩子、103/198 业务回填钩子、140/371 条件执行）——**413 个迁移在全新库上全部跑通，幂等验证通过**
- [x] **S2c. sqlx 离线缓存**：`.sqlx` 已生成，SQLX_OFFLINE 构建验证通过；user.rs 查询模块用 query_as! 宏建立编译期校验模式 + 真实 DB 集成测试
- [x] **S2d. db 层全量**：生成器脚本（`server-rs/scripts/gen_db_port.py`）解析 sqlc 生成的 Go 代码，批量移植 **54 个查询模块 / 848 个查询**（305 one_model + 116 many_model + 38 one_row + 75 many_row + 202 exec + 80 scalar_one + 32 scalar_many），models.rs 从活库 information_schema 生成（100 表、可空性准确）。位置提取与 Go Scan 顺序同构；`generated_user_queries_roundtrip` 集成测试在真实 DB 上验证往返。修复了三个生成器缺陷：闭包内 `?`（3341 错误→for 循环）、f-string 括号折叠、可空性二次比较丢失 Option（会导致运行时 NULL 解码崩溃）
- [x] **S3. config 全量对齐**：cordy-config 覆盖 cmd/server 全部 44 个环境变量 + auth/storage 启动变量（JWT/AWS/CloudFront/Resend/SMTP/LLM/Lark/WeCom/Composio/entitlement/fleet），变量名与 Go 完全一致；TOML+env 双层加载、is_production() 安全门控、类型化解析校验（CORDY_LLM_MAX_RETRIES u32）。修复两个测试缺陷：parses_toml_file 缺 ENV_LOCK 并行竞争、ServerConfig derive(Default) 使 port 默认 0（Go 为 8080）
- [x] **S4a. 探索完成**：auth 包 8 文件（jwt/cookie/cloud_pat/cloudfront/pat_cache/membership_cache/daemon_token_cache/temporary_disabled_users）+ middleware 包 10 文件结构已探明；Auth 中间件四种认证路径（X-User-ID 直通 / mat_ 任务令牌 / mcn_ 云 PAT / mul_ PAT / JWT）流程完整记录
- [x] **S4b. cordy-auth 移植**：jwt.rs（secret OnceLock 管理 + ValidateJWTSecret denylist + mul_/mdt_/mat_ 三种 token 生成 + SHA-256 HashToken）、cookie.rs（cordy_auth/cordy_csrf 常量 + Go duration TTL 解析器 + COOKIE_DOMAIN IP 拒绝 + FRONTEND_ORIGIN scheme 推导 Secure + CSRF HMAC 绑定生成/验证）、disabled_users.rs（紧急禁用名单）、pat_cache.rs（Redis ConnectionManager 缓存，nil 安全语义，TTLForExpiry 钳制）
- [x] **S4b+. 缓存三件套补齐**：membership_cache.rs（mul:auth:member:{user}:{ws}，5 分钟 TTL，仅存成员存在性不存角色）、daemon_token_cache.rs（mul:auth:daemon:{hash}，DaemonTokenIdentity 序列化字段名 w/d 与 Go 字节级兼容）。disabled() 构造器对应 Go nil 指针安全语义。**cordy-auth 共 19 个单元测试**
- [x] **S4c. axum 中间件移植（核心三件）**：新建 crates/cordy-middleware——
  - auth.rs：Auth 中间件完整移植（X-Actor-Source 防伪造剥离 → X-User-ID Clerk 直通 → extractToken Bearer>cookie → CSRF 门控（非 GET/HEAD/OPTIONS）→ mat_ 任务令牌（注入 X-Agent-ID/X-Task-ID/X-Workspace-ID/X-Actor-Source=task_token）→ mcn_ fail-closed → mul_ PAT（缓存+TTLForExpiry 钳制+异步 last_used_at）→ JWT HS256（required_spec_claims 清空+leeway 0 对齐 golang-jwt v5 语义））；禁用用户检查贯穿所有分支
  - workspace.rs：四种守卫变体统一为 WorkspaceGuardState 配置化中间件（member_only/with_roles/from_url/from_url_with_roles），slug-first 解析链 + 任务令牌绑定双重校验（MUL-2600）+ WorkspaceContext 扩展注入；FromURL 变体用 MatchedPath 提取路径参数（需 route_layer 挂载）
  - daemon_auth.rs：DaemonAuth 完整移植（mdt_ 主路径 daemonCache 短路 + mcn_ fail-closed + mul_/JWT 回退共享 patCache，DaemonContext 扩展携带认证路径标签用于慢日志归因）
  - **剩余**：cloudfront.go（依赖未移植的 CloudFrontSigner）、owner_lookup.go（依赖 cloud_pat.go verifier 类型）——随对应模块移植
- [x] **S4d. 剩余小中间件移植**：csp.rs（CSP 双变体：标准/附件预览 frame-ancestors 差异 + 路径判定）、client.rs（X-Client-* 头提取到 ClientMetadata 扩展，best-effort 语义）、plugin_auth.rs（mpi_/mpc_ 前缀路由：插件令牌直达 handler、其余走 Auth 链；BearerToken 大小写不敏感解析）、ratelimit.rs（Redis Lua 原子 INCR+EXPIRE 防永久 ban、可信代理 CIDR 列表、XFF 右到左遍历取最右非可信 IP、429+Retry-After 手工构建响应）、request_logger.rs（webhook 路径脱敏 [redacted]、soft-404 分类——"runtime/task not found" 降为 Info 避免告警洪水、仅 404 时捕获 body 分类其余状态流式直通、内部 trigger-ID 响应头读取后剥离）。**workspace 总计 47 个测试全过**
  - 修复过程：HttpBody 无效导入、tracing event! 动态 level 改 macro_rules! token 透传、serde_json::Value 裸值无 IntoResponse 改手工构建响应、needless_lifetimes ×2
- [x] **S5a. events 总线移植**：新建 crates/cordy-events——bus.rs 完整移植（Event 结构含 task_id/chat_session_id scope hints（MUL-1138）、typed+global 双层订阅、注册顺序同步分发、panic 隔离（catch_unwind + AssertUnwindSafe，单个 handler 崩溃不影响其他 handler））。**6 个单元测试全过**（类型过滤/注册顺序/global 后置/panic 隔离/空发布安全/scope hints 传递）
- [x] **S5b-a. realtime 基础三件移植**：新建 crates/cordy-realtime——
  - broadcaster.rs：SCOPE_* 五常量 + Broadcaster trait（改 async：MirroredRelay 双发需 await Redis I/O，Go 的阻塞语义在 Rust 对应 async trait）+ DaemonRuntimeDeliverer + RelayPublisher（async-trait 支持 dyn 兼容）
  - metrics.rs：Metrics 完整移植——7+16 个原子计数器、CounterMap 动态计数表（sync.Map → Mutex<HashMap<String, Arc<AtomicI64>>> load_or_init 模式）、RedisStreamObservation、Snapshot JSON 键名与 Go 字节级一致、Reset 清零、LazyLock 全局 M 单例
  - relay_lifecycle.rs：ManagedRelay trait（CancellationToken 替代 Go context）+ MirroredRelay 双发灰度助手（daemon_runtime scope 只发 primary、镜像 primary/secondary 错误与 divergence 指标追踪、errors.Join 等价合并、ULID 事件 ID）
  - **cordy-realtime 9 个单元测试全过**
- [x] **S5b-b. stream_retention 移植**：StreamRetentionConfig（TTL 故意 opt-in 的灰度设计 + with_defaults 顺序敏感修正链：零值填充 → TTL<trim 补 replay grace → 无效 interval 回退 ttl/3 或保留有效偏好值）、StreamTtlRefresher（claim/release 抢占模式限制 PEXPIRE 频率、reconcile_ttl 双路径——enabled 修复缺失 TTL / disabled PERSIST 移除 TTL 回滚兼容、可注入时钟便于测试）、辅助函数（stream_min_id 毫秒格式+负数钳制、redis_info_int64 解析 INFO 输出、redis_ttl_millis 哨兵透传）。**10 个单元测试全过**；修复了两个测试断言错误（UnixMilli 毫秒语义、retentionSubinterval 有效偏好优先语义）并新增顺序敏感用例
- [x] **S5b-c-a. sharded_stream_relay 移植**：ShardedStreamRelay<H: HubFanout> 完整移植——FNV-1a 分片路由（scopeType+NUL+scopeID）、XADD MAXLEN~ 发布路径（指标+TTL refreshIfDue）、每分片独立连接的 XREAD BLOCK 读循环（replay grace 起步 + generation 代际重置机制）、retention 维护循环（EXISTS/XTRIM MINID/reconcileTTL/XLEN/MEMORY USAGE/observe + INFO memory/stats 服务器观测）、心跳循环（SET heartbeat EX 90s）、stream presence 缺失检测（generation++ 触发 replay 重置）。Hub 解耦：新增 HubFanout trait（fanout_all_dedup/fanout_user/broadcast_to_scope_dedup），deliver_envelope 改为接收拥有句柄支持 tokio::spawn。关键设计：XREAD BLOCK 需要独立连接（多路复用连接会被阻塞饿死）——reader 从 read_client 各自建连，写路径共享 ConnectionManager clone 句柄（无锁跨 await 安全）。**7 个新测试（含 XREAD Value 树解析器），cordy-realtime 共 34 测试**
- [x] **S5b-c-b. redis_relay 移植**：RedisRelay 完整移植——ScopeKey/ScopeEventCallback/ScopeSubscriptionSource trait（Hub 解耦：LocalScopes 快照 + 订阅变化回调）、per-scope consumer 管理（HashMap<ScopeKey, ConsumerHandle> 去重 + CancellationToken 生命周期 + tasks 表 wait 汇合）、run_consumer（XREADGROUP GROUP COUNT 32 BLOCK 5s + NOGROUP 自修复：forget TTL→重建 group 0-0→refreshIfDue→MissingTotal++ + XAck after delivery + XGROUP DELCONSUMER 退出清理）、heartbeat（SET EX 90s + ZAdd NodesKey 刷新所有本地 scope）、consumer_sweeper（ZREMRANGEBYSCORE 过期节点清理 + SCAN ws:scope:*:stream 游标遍历维护非本地流 + completeLegacyTTLScan 剪枝）。复用 parse_xread_response（改 pub）。**全 workspace 87 测试全过**
- [x] **S5b-d. hub.go 移植**：Hub 核心（1071 行 Go → hub.rs）——ClientId/ClientHandle/DedupCache（LRU 128 mark_seen 去重）、ScopeAuthorizer/PatResolver/MembershipChecker traits、Hub 结构体（RwLock<HubInner> 替代 Go channel 事件循环；subscriptions 由 hub 写锁保护与 Go hub.mu 契约一致）、register（自动订阅 workspace+user scope）/unregister（rooms 清理+onLast 回调+指标）、subscribe/unsubscribe（0↔1 边界触发 onFirst/onLast）、broadcast_to_scope_dedup/fanout_all_dedup/fanout_user（markSeen 去重 + try_send 非阻塞投递 + 慢客户端收集）、evict_slow（队列满驱逐 + drained rooms 回调）、has_local_subscribers/local_scopes/snapshot/authorize_subscription。**实现 HubFanout + ScopeSubscriptionSource 两 trait 完成解耦闭环**。WS 泵（readPump/writePump/auth 升级握手）随 axum handler 层移植（S8）。**6 个新测试，cordy-realtime 共 40 测试，全 workspace 93 测试全过**
- [x] **S6. service 层**：按域逐个移植（email/cron/issue/plugin...）——全量完成（26 文件清零，详见收尾审计表）
  - [x] 探索：service 包实际 **30,012 行非测试代码**；task.go 6,710 最大、autopilot.go 1,941、plugin.go 854+plugin_hook.go 569、issue.go 765+issue_trigger.go 218、chat_quick_actions_generate.go 497、email.go 434、cron.go 138、builtin_agents.go 80+builtin_skills.go 72
  - [x] cron.go → crates/cordy-service/src/cron.rs（**croner + chrono-tz**；五年 horizon 年份截断对齐 robfig Next() 零时间语义；TZ=/CRON_TZ= 缺空格 panic 防护——robfig parser.go:99 slice[:-1] bug；NextOccurrencesUTC 半开区间 (after,until] + 1024 硬上限；ComputeNextRun 本地时钟仅显示用注释说明）— 7 测试过
  - [x] builtin_agents.go → src/builtin_agents.rs（MIKA_SYSTEM_KEY/DEFAULT_NAME 常量；include_str! 相对路径引用 Go 侧 INSTRUCTIONS.md 保持单一来源；{{AGENT_NAME}} 占位符替换；MIKA_WORKSPACE_NOTES_SECTION 分层组合，空笔记直接返回系统半区）— 5 测试过
  - [x] **cordy-service 共 83 测试，全 workspace fmt/clippy -D warnings/176 测试全过 0 失败**（本轮 +33：chat_quick_actions 10 / task_failure 15 / task_helpers 8；relay_lifecycle 全局指标竞争修复）
  - [x] email.go → src/email.rs（手写 async SMTP 客户端对齐 net/smtp 子集语义：PLAIN→LOGIN 回退决策纯函数化、PlainAuth 明文拒绝守卫、textproto dotWriter 逐字节等价含裸 LF→CRLF 规范化、8BITMIME 探测+quoted-printable 回退、RFC 2047 Q 编码带 75 字节折叠、Go html.EscapeString 五字符转义、subject 60 rune 截断+省略号、Resend reqwest 直调 API）— 14 测试过；新增依赖 reqwest/tokio-rustls/rustls/rustls-native-certs/base64/quoted_printable/hostname
  - [x] **cordy-service 共 26 测试，全 workspace fmt/clippy -D warnings/119 测试全过 0 失败**
  - [x] **cordy-db 事务支持改造（关键基建）**：gen_db_port.py 签名 `pool: &sqlx::PgPool` → `executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>`，848 查询重新生成——同一函数既可传 `&pool` 也可传 `&mut *tx`；user.rs 手写模块同步改造。修复重复守卫两查询的可空参数语义（project_id/parent_issue_id → Option<Uuid>，生成器无法从 pgtype.UUID 看出 nullability，按需手改模式确立）
  - [x] dispatch/reason.go → src/dispatch_reason.rs（ReasonCode 枚举 13 值，serde snake_case 与 Go 字符串常量字节级一致）— 2 测试
  - [x] issueposition → src/issue_position.rs（next_top_position 泛型 Executor）— 1 集成测试（真实 DB 验证位置递减/空列 -1）
  - [x] issueguard → src/issue_guard.rs（normalize_title 对齐 SQL 侧 lower(btrim(regexp_replace)) 表达式、ActiveDuplicateError、lock_and_find_active_duplicate/recent_autopilot_duplicate 强制事务作用域 executor——advisory xact lock 用裸 pool 会立即失效）— 4 测试
  - [x] issuestatus → src/issue_status.rs（7 内置状态=7 类别一一对应模型、effective 内置免查询快速路径+fail-safe 方向、resolve 内置 fail-open 语义、Resolver 懒加载摊销+失败也缓存、expand_categories 保索引访问路径、validate_key/slugify_key 纯函数）— 6 测试含集成
  - [x] builtin_skills.go → src/builtin_skills.rs（include_dir 嵌入 Go 侧目录保持单一来源）
  - [x] **cordy-realtime 测试修复**：relay_lifecycle 全局指标 M 并行 reset 竞争（lifecycle_forwards 的 reset 抹掉并行 join_reports 刚累加的计数）→ METRICS_LOCK（tokio::sync::Mutex，async 测试 lock().await / 同步测试 blocking_lock()）串行化；clippy 1.97 新 lint 批量清理（manual_contains/write_with_newline/Some+?/hub.rs 未用变量）
  - [x] chat_quick_actions_generate.go 纯函数核心 → src/chat_quick_actions.rs（上下文选择 select/answered 集合在 kind 过滤前从原始行播种、previous 标签收集 ToLower 去重 newest-first cap 6、渲染布局逐字节对齐、rune 截断 head2000+tail1000 双端保留、ChatQuickAction serde 对齐 Go json tag 含 primary omitempty、ChatQuickActionsLlm async-trait 接缝）；TaskService 方法体随 task.go 落地 — 10 测试
  - [x] **叶子包批次①完成**：
    - pkg/taskfailure 全量 → src/task_failure.rs（Reason 开放集合 Cow<'static,str> 新类型——normalize_daemon_reason 透传旧值如粗粒度 "agent_error"，封闭枚举不忠实；classify 13 规则顺序=SQL CASE 源真逐条对齐含 token&&limit 组合与 curly apostrophe；HTTP 码数字边界正则防 "402913 tokens"/"exit status 4030" 误分类；unresumable_history 双信号必须同时（emptiness 抱怨 AND message locator）；auth_method_unresolved/provider_unconfigured 精确短语；context_exhausted_completion 三复合子句+320 字节上限且裸 "Prompt is too long" 故意不匹配；normalize_daemon_reason 四条 legacy daemon 升级规则）— 15 测试。新增 workspace 依赖 regex = "1"
    - pkg/dbid → cordy-db/src/dbid.rs（NewV7 mint 契约：仅行身份用途、DB COALESCE 回退保留在 SQL 侧、Rust now_v7 无错误路径故回退分支不可达已注释；进程内单调测试 ×100）— 2 测试
    - pkg/redact → src/redact.rs（15 个 secret 模式顺序敏感首个命中生效、AIza 尾分隔符捕获还原 $1、深度上限 32 超限整体丢弃占位符 fail-safe、serde_json Value 统一遍历覆盖 Go 的 []string/map[string]string 特例、home 目录掩码 HOME/USER env 版、**通用凭证正则故意无词边界**——MY_TOKEN= 中间匹配产出 MY_[REDACTED CREDENTIAL] 与 Go 一致）— 9 测试
    - internal/attribution → src/attribution.rs（MUL-4302 问责链纯分类：Source 开放字符串新类型含 precise() 五合规级判定、finalize_attribution 单向不变量 originator 非 NULL⟹accountable 相等、classify_comment 三分支（member 直认/agent 链行走 originator→accountable 复制→无源任务 unattributed）、classify_direct actor 优先于 creator、rule_owner/trigger_owner 审计-授权两列分歧（user_id NULL 而 accountable 有值）、owner_fallback 仅从 unattributed 降级且永不伪造、delegated_subscriber 瀑布 quick_create=creator/agent_create=delegated/autopilot 排除）— 13 测试
    - internal/featureflags/keys.go → src/feature_flags.rs（7 键常量+4 个 Enabled 助手默认 false fail-closed+evaluate_frontend_public_flags 含三个永久 true 的 compat 键；FlagSource trait 接缝替代 *featureflag.Service——**pkg/featureflag 本体 1,826 行仍未移植**，落地后实现该 trait 即可）— 3 测试
  - [x] task.go 包级助手 → src/task_helpers.rs（truncate_for_summary rune 截断+空白扁平化、truncate_fallback_comment_body 超限整体替换固定通知 GH#5455、is_trivial_done_output 六语言标记、retry_attempt_ceiling 只放宽 provider_network 且 max_attempts<=1 永不复活、retry_delay_for_attempt runtime_offline 正 fire_at/provider_network 末次 5s、resume_unsafe_failure 毒集+400 invalid_request_error 文本守卫+auth_method_unresolved+unresumable_history 四层、retry_eligible、has_runnable_successor 接生成查询、compute_chat_elapsed_ms 用 created_at 且负值钳零、priority_to_int）— 8 测试
  - [x] builtin_skills.go → src/builtin_skills.rs（include_dir! 编译期嵌入 Go 侧目录树单一来源；AgentSkillData/FileData serde 字段名对齐 Go tag 含 omitempty；include_dir 路径按嵌入根解析的坑已记录）— 4 测试
  - [x] **task.go 结构地图完成（explore agent ×2 轮）**：169 函数/TaskService 15 字段/7 功能簇；切片边界 976/2187/3695/(5132)；纯函数锚点先行。**二轮补充**：issue.go/autopilot.go/plugin.go 消费而非扩展 TaskService（仅 chat_quick_actions*.go(7)+builtin_skills.go(1) 挂方法）；**Hub 字段是死的**——task.go 全部事件走 s.Bus(18 处)，s.Hub 零引用，Rust 结构体可省 Hub 句柄；字段用量 Queries=106/Metrics=21/Bus=18/runInTx=13/EmptyClaim=7/TxStarter=3/Composio=3/FeatureFlags=2/Wakeup=2/QuickActions=1；最大函数 FailTask(~400)/CompleteTask(~225) 各需独立审查
  - [x] **S6 执行序（依赖拓扑）**：~~① 叶子包批次✅（taskfailure/dbid/redact/attribution/featureflags-keys；runtimeapps+skillbundle 更早已移植；pkg/featureflag 本体 1,826 行待补）~~ →  - [x] **② protocol(602) → 新 crate cordy-protocol**（events.rs 全部 ~90 个 WS 事件常量字节级对齐；messages.rs 全部 ~30 个 payload 结构体 serde 字段名与 Go json tag 一致：值类型 omitempty 用 String::is_empty/泛型 is_zero 精确模拟 Go 空串零值省略、指针 omitempty 用 Option+is_none、ChatSessionUpdatedPayload.project_id 的 Go **string 双指针用 Option<Option<String>> + 自定义 double_option serde 区分「未触碰/显式 null/有值」、type 字段 rename、Message envelope payload 用 serde_json::Value 对应 json.RawMessage）；chat_quick_actions.rs 重构复用 protocol::ChatQuickAction 消除本地重复 — 9 测试（含 omitempty 省略形状断言、双指针三态 roundtrip、heartbeat_ack 嵌套可选全形状往返） → [x] **③a metrics 业务半区（~2,100 行）→ 新 crate cordy-metrics**：
  - **前置解耦**：taskfailure 从 cordy-service 提取为独立 crate cordy-task-failure（cordy-service 留 glob re-export shim 保持路径稳定）——labels.go/business.go 都 import pkg/taskfailure，否则 cordy-metrics→cordy-service 循环依赖
  - labels.rs：60 条 metric 标签表数据驱动（metric_labels panic on missing 对齐 Go）、9 个高基数禁用标签守卫 validate_business_metric_labels、Normalize* 核心（failure_reason 大小写敏感精确匹配→classify 回退；model_alias ASCII 白名单替换+128 字节截断）
  - labels_pr3.rs：19 个 allowlist 归一化、signup_source JSON cookie 解析（utm_source/source/referrer/ref 键序回退）+ URL 主机别名归一（twitter.com/x.com/t.co→twitter 等 25 条）、cloud runtime 状态三字符首字节分桶 {ok,4xx,5xx,timeout,error}
  - pricing.rs：32 行价格表（GPT-5.6 系列 cache write 1.25x 特例、Sonnet 5 限时价、xAI 短上下文档故意低估 ≤50% 而非高估 100%、grok-composer-* 故意不映射）、32 条别名正则规则（锚定 $ 拒绝变体、点号字面量不做 dash 归一）
  - business.rs：BusinessMetrics ~30 collectors、record_task_* 全套（dispatch 记 in-progress 去重表+terminal 清除、attempt<1 钳 1）、record_llm_usage 双路径（无价行→unpriced 四桶+provider 报价整落 input 桶；有价行→rate 分摊+authoritative 总额按比例重缩放保持总额精确）、prewarm 6×3×25 失败原因网格、register_all 注册 /metrics 端点
  - business_events.rs：35 个 PR3 漏斗计数器+typed Record* 助手（含 task.go 所需 record_chat_output_local_path——闭枚举防路径片段泄漏进 Prometheus）；**RecordEvent/IncForEvent PostHog 配对桥随 analytics 包延后**
  - prometheus crate API 适配：Counter 用 inc_by 非 add、Histogram::with_opts、MetricFamily.name() 替代废弃 get_name — 31 测试 → [x] **③b analytics(1,358 行 Go)→ 新 crate cordy-analytics**（client.rs trait AnalyticsClient{capture 非阻塞/close 排空}+Event 结构+NoopClient+new_from_env 环境变量装配（POSTHOG_API_KEY 空→noop、ANALYTICS_DISABLED 强制 noop、ANALYTICS_ENVIRONMENT/APP_ENV 归一 production/staging/dev）；events.rs 25 个导出构造函数与 Go 一一对应（Signup/WorkspaceCreated/Runtime*×4/AutopilotRun*×3/Issue*/ChatMessageSent/TeamInvite*×2/Onboarding*×4/AgentCreated/CloudWaitlistJoined/FeedbackSubmitted/ContactSalesSubmitted/SquadCreated/AutopilotCreated）+withCoreProperties 空值省略/is_demo 恒写语义对齐；posthog.rs /batch/ 端点有界队列非阻塞 capture+后台批量 flush+丢弃计数 — 21 测试；**配对桥补齐**（cordy-metrics/business_events.rs）：record_event（metrics-only 跳过 PostHog capture 但计数器照加，MUL-4127 契约）+ inc_for_event 全 23 分发臂（runtime_ready 附带 ready_duration_ms>0 秒级 observe、runtime_failed 四标签含 bool_label(recoverable)、autopilot_run_completed/failed 落 terminal 计数器）+ string/int64/bool_prop 访问器 +2 桥接测试） → [x] **③c metrics 基建半区（http/db/realtime/channel_lease/channel_media/wecom/config/server/sampler/registry ≈1,700 行）**：
  - http.rs：HttpMetrics（requests/duration/in_flight/daemon_workspace_response_size 指数桶）+ axum 中间件（健康探针路径旁路、MatchedPath 路由标签 unmatched 回退）
  - db.rs：DbCollector（sqlx pool size/num_idle 映射 total/idle/acquired_conns；pgx 独有计数器无 sqlx 对应物，缺失即不上报而非置零防假平线）
  - realtime.rs：RealtimeCollector 完整移植 cordy_realtime::Metrics 25 个 const metric（mirror errors 按 target=primary/secondary 双序列、stream observations 按 stream 标签三序列），指标名与 Go 字节级一致
  - channel_lease.rs / channel_media.rs / wecom.rs：三组业务计数器直译（WeCom connect_failures 与 auth_failures 故意分离的注释语义保留）
  - config.rs：METRICS_ADDR 解析 + is_loopback_addr（SplitHostPort 回退链+IPv6 括号剥离+localhost 大小写不敏感）
  - server.rs：独立 /metrics axum 服务（TextEncoder，gather 失败降级纯文本错误）
  - sampler.rs：BusinessSamplerCollector 全量移植（8 条 SQL 各自短只读事务+SET LOCAL statement_timeout+LIMIT 100、TTL 缓存吸收并发 scrape、失败保留旧快照、57014 超时 INFO 其余 WARN、心跳年龄直方图 Rust 侧分桶、source/runtime_mode/provider 白名单归一复用 labels 模块）；Go 同步 Collect → Rust block_in_place+current_thread runtime 驱动 async DB
  - registry.rs：RegistryOptions{pool,realtime,version,commit,sampler} 组装全部 collector + cordy_build_info
  - 修复中断会话遗留：cordy-analytics 缺 serde 依赖连锁编译错、PostHogConfig 未导入路径、空标签切片类型标注 ×6、events 测试断言与 Go withCoreProperties 空值省略语义矛盾（改断言非改实现）、record_event 配对测试 signup_source "x" 应归一为 "twitter" 桶、clippy too_many_arguments ×2 — **全 workspace 274 测试全过，clippy -D warnings 干净，fmt 已格式化** → [x] **④ task.go Slice1(1-975) → cordy-service/src/task_service.rs**：TaskService 结构体（pool+bus+analytics/metrics/wakeup/feature_flags/composio/quick_actions 可选接缝；Go 死字段 Hub 省略、quickActionsInFlight/Running 并发门、analyticsContext LRU 4096）；ErrAttributionFailClosed/ErrDuplicatePendingTask 哨兵 + is_duplicate_pending_task_err（23505 双索引名）+ pending_slot_taken_err 双形状匹配；归因族全量（buildCommentTriggerSummary MUL-4252 workspace 范围、resolveOriginatorFromTriggerComment/attributionFromTriggerComment/attributionFromComment 走 source_task_id→parent 链、AttributionForMergedComment isMention→delegation 标签、attributionForIssueTask actor>comment(跳过 system)>autopilot origin→triggerOwner>DirectFacts(origin 继承 quick_create/agent_create)、applyAttributionFallback fail-closed 三拒绝路径、ruleOwner/triggerOwner 自由函数）；buildRuntimeMCPOverlay feature-flag 门控+失败软降级；parseQuickCreateContext 无链接前置校验；taskAnalyticsContext 缓存优先+runtime→agent 回退+issue/chat/run 四路覆盖+quick_create 终写；captureTask* 七个 metrics 助手含 cancel 的 mat_ token 撤销(MUL-2600)；durationSeconds 反向钳零/taskErrorType 桶表 — 7 新测试（fixture 手写 52 字段结构体字面量因 models 无 Deserialize），**全 workspace 281 测试全过 clippy/fmt 干净** → [x] **⑤ Slice2(976-2186 入队族)**：enqueueIssueTaskWithCommentPlan（assignee/archived/runtime 三守卫、attribution→fallback→overlay→createParams 管线、fireAt 分流 CreateDeferredChannelIssueTask/CreateAgentTask、queued 广播先于 daemon 唤醒的 observe-order）；EnqueueTaskForMention/ThreadParent/SquadLeader(+Handoff)（delegation 标签、is_leader_task+squad_id、23505 双索引名→ErrDuplicatePendingTask 裸哨兵 #5914）；EnqueueDeferredAssigneeFallback（创建时盖归因章防 NULL-source bypass MUL-4302 §2）；EnqueueQuickCreateTask（context JSONB payload、direct_human 无证据对特例）；hydrateDeferredChannelIssueTaskOverlay（expected_originator 条件更新防 merge 竞态）；PromoteChannelChatTasksIfMediaReady/PromoteDeferredChannelIssueTask（ErrNoRows 幂等）；CancelTasksForIssue（MUL-4465 仅限 issue 生命周期清理）；事件发布族 taskEvent/broadcastTaskDispatch（context JSONB 展开+chat_session_id 路由键）/broadcastTaskFailedEvent（retry_pending 抑制 error 字段+redact）；ResolveTaskWorkspaceID 四级回退含 quick-create context；NotifyTask* 族（bump-before-wakeup 顺序注释保留）；EnqueueChatTask（FOR NO KEY UPDATE 锁序 chat_session→binding 防 ABBA、pending_fresh 提升 forceFreshSession、media deadline 读后 seal 再重推导 defer）；SendDirectChatMessage（overlay+归因在事务外解析、锁内重读 session+agent、HasPendingChatTurnForSession 位置语义、chat_input_task_id 自持有、AdoptOrphanOnboardingKickoff 先于用户消息、附件绑定、touch）；OpenMikaOnboardingChat（kickoff+opening 双行事务、ChatSessionHasUserMessage 防重）；RegenerateChatQuickActions（latest assistant turn 校验+stale 拒绝+busy 双闸）— TaskServiceError 扩至 14 变体（产品化哨兵与基建错误分层），anyhow→sqlx downcast 桥接生成查询层 — **全 workspace 281 测试全过 clippy -D warnings/fmt 干净** → [x] **⑥ Slice3(2187-3694 取消+认领)**：CancelTasksForIssue 补 distinctAgentIDs 去重 reconcile（D#3319）；CancelTasksForAgent/ByTriggerComment；BroadcastCancelledTasks（workspaceID 由调用方传入防已删行解析失败）；CancelTask/ByUser/WithReason→CancelTaskWithResult（SanitizeTextForPostgres 防 NUL 回滚 GH#7098、user_initiated 禁带 reason、QueuedOnly CAS、ErrNoRows 幂等返回现有行、chat_session→task 全局锁序）；CancelQueuedChatTasks（session FOR UPDATE→agent 锁序）；settleQueuedChatInput（channel_ingested→"Stopped." 行 vs 删除输入+edit 分离附件+draft restore）；finalizeCancelledChatMessage（空 transcript 三分支：channel 不可恢复/started+支持客户端→MarkChatFinalizeDeferred 延迟判定 #5219/否则 detach→delete→restore）；FinalizeDeferredCancelledChat（锁内原子 claim marker 防双 finalize、session gone 容忍、restored/stopped 双 outcome 广播 chat:cancel_finalized）；RebroadcastCancelledTask（幂等重播）；ReconcileAgentStatus/publishAgentStatus；claimTask（FOR UPDATE agent→容量检查→ClaimAgentTask prepare lease→direct-chat reanchor 兼容回退→300ms 慢日志）；ClaimTaskForRuntime（promote→stale reclaim 先于 empty cache→候选列表→按 agent 去重尝试）；ClaimTasksForRuntimes 批量版（MUL-4257 六步语义+部分成功返回防双 claim）；cancelSupersededDeferredRetries（23505 容忍跳过一个 tick）；PromoteDueDeferredTasksForRuntime；RequeueTaskAfterClaimFailure（CAS dispatched_at）；FinalizeTaskClaim（token+daemon token 过期清理+delivered comment ids 回执单事务）；StartTask（含 cancelDeferredEscalationsForTask）— **全 workspace 281 测试全过 clippy -D warnings/fmt 干净** → [x] **⑦ Slice4a(3695-5131 终态+重试) + 4b(5132-6710 委托恢复+通知/映射)**：cordy-service 新增四模块（task_terminal/task_recovery/task_notify/task_quick_actions 共 3,394 行）——
    - task_terminal.rs：CompleteTask（事务内 status CAS + resume-pointer COALESCE 推进 + retired 清除 GH#6066 + assistant outcome 同事务 MUL-4351；UPDATE 无行→幂等返回现有行；issue 回退评论 = no_action 评估抑制 × HasAgentCommentedSince 门控 × trivial 输出抑制，redact 先于截断 GH#5455；chat:done 提交后广播 + quick-actions 自动 pass）；FailTask（SanitizeTextForPostgres 前置、classify→NormalizeDaemonReason 双层守卫 MUL-2946/MUL-5370、auto-retry 事务外预计算（Composio overlay 仅 retryable 才花）、事务内 retry-child 原子创建 + ON CONFLICT 让位语义、resume-unsafe 双信号清指针 / retired 无条件清 / 安全路径重推进三分支、非 retried 写可见失败行并释放 onboarding kickoff MUL-5827、delegated 恢复钩子、系统评论与 quick-create 失败通知均被 retried 门控）；MaybeRetryFailedTask（sweeper 镜像：reason-aware ceiling 使孤儿 provider_network 保住第三跑 MUL-4910、successor 检查失败仍尝试的容忍语义）；RerunIssue（source-task 解析带 leader/squad 血统 + trigger 继承 + promoteNewestSurvivingComment 修复已删触发、canInvoke fail-closed 先于一切变更 MUL-4525、pending-slot 清理与双提交竞态单次重试 #5914、rerun_of_task_id 入插）；HandleFailedTasks（retry-first 防 todo 抖动、stuck in_progress 重置排除 in_review/blocked 且直写补发 issue:updated MUL-6243/#4648、按 agent 合并 reconcile）
    - task_recovery.rs：委托失败恢复子系统全量——形状校验、content 文案（go_quote 对齐 strconv.Quote 子集）、loadTarget 七重否决门、ensureComment 行锁串行 + 幂等查找、exhaust（AcknowledgeExhausted 原子认领、第二 sweeper 观察到首个解释不重复上报、故意不 @mention coordinator 防第四次恢复、recipient 非 member 容忍）、dispatch 三趟循环（covered → count≥3 exhaust → merge 进 pending coordinator → CreateAgentTask successor → 23505 转 RegisterPlannedCommentForActiveTask 记 planned-but-undelivered 交完成对账）、RecoverPendingDelegatedFailures outbox 重放（errors.Join 等价聚合）
    - task_notify.rs：ReportProgress / updateAgentStatus / loadAgentSkills(+Bundles，BuildAgentSkillBundles 与 AgentSkillRefData serde tag 对齐) / broadcastChatDone / broadcastIssueUpdated / getIssuePrefix / createAgentComment（thread-root 解析不覆写 parent_id + escalation 取消 + 仅此路径携带 revision）/ AutoUnresolveThreadOnReply(#2300) / builtInStatusCategory / IssueToMap(+WithCategory MUL-6243) / quick-create 收件箱三态（origin 确定性查找、lookup 失败→unconfirmed 中性通知防静默丢弃、CLI 错误文本优先 #5885、failed/unconfirmed inbox type 分离防 Failed: 前缀误报）/ publishQuickCreateInbox / agentToMap
    - task_quick_actions.rs：GenerateChatQuickActionsForTask（target 先锚定防异步错位、非普通 turn 转占位符解析）+ Async（&Arc<Self> detached spawn、session 单飞 entry-API、进程级 ceiling=16 shed、timeout 包裹）/ SupplementChatQuickActions（parse→逐字段 redact→有 actions 写回否则读现值→chat:quick_actions 广播 failed 标记）/ eligible（channel_ingested 判别 + no_response/附件-only 排除）/ in-flight 查询 / resolvePlaceholder；parse 三级回退（raw→fence→bracket-span 顺序敏感）+ inside_code_fence + unmarshal（actions 包装或裸数组）；ChatQuickActionsLlm 统一为 Go 形状（enabled+generate_json）替换旧 generate() 缝隙
    - 基建修复：cordy-util 补 unescape_backslash_escapes（\n\r\t\\ 四序列 + 尾反斜杠透传）+1 测试；create_comment 可空参数手改 Option<Uuid>（生成器参数元数据丢失第二例）；update/clear_chat_session_session 的 runtime_id Uuid→Option<Uuid>（COALESCE 配裸 Uuid 会写入零值 UUID 腐化路由键——不改会静默写坏）；issue_status::effective 去 Copy 放行 &mut PgConnection 复用
    — **全 workspace fmt/clippy -D warnings 干净，363 测试全过（基线 362→363）** → [x] ⑧-a issue.go 前置改造：task_service 入队管线拆出 prepare_issue_enqueue（守卫+归因+触发摘要+review sha 元数据阶段，无写入；build_overlay 门控使事务路径不跨网络调用持锁——Go txService nil-Composio 语义对齐）+ 新增 create_deferred_channel_issue_task_tx 事务作用域变体（镜像 Go `txService := &TaskService{Queries: q}` 技巧，供 IssueService::create 让 media-gated deferred task 与 issue 行同事务原子提交） — clippy/fmt 干净 368 过
 → [x] **⑧-b 新模块 issue_service.rs（issue.go 全量+issue_trigger.go+runtime_unusable_notice.go）**：
    - 类型层：IssueCreateParams/Opts（BroadcastPayload 以 Arc<dyn Fn> 钩子注入 handler 响应形状）/Result；五哨兵并入 IssueCreateError 枚举（ActiveDuplicate 携 Box<Issue> 冲突行供调用方渲染冲突，Parent/Project/LabelNotFound 单漏斗语义保留，StatusUnavailable=目录归档竞态）；RunEnqueueSource/IssueTriggerProbe(ProbePredicate<Agent|()> 类型别名消生命周期)/IssueTriggerInput/IssueRunTrigger
    - create 七步事务管线逐条对齐：自定义 status 目录共享锁+resolve 重解析（归档竞态→StatusUnavailable，内建跳过）；parent 校验含 project 回填、project 校验单漏斗（Go 连瞬态故障也读作 not-found）；labels 先于 counter 校验（廉价失败）+AttachLabelToIssueOnCreate 同事务循环；LockAndFindActiveDuplicate 命中→ActiveDuplicate{Some(dup)}；counter→NextTopPosition（注释保留 workspace 行锁顺序论证）；CreateIssueWithOrigin/CreateIssue 按 origin 分流；fire_at 时 tx 内建 deferred channel task（经子片⑧-a 的 tx 变体，issue 行可见者必见惰性任务的唯一索引确定性）→提交；
    - 提交后序列：linkAttachments best-effort（错误降级日志不失败）→actor 回退 creator→fire_at 路径 hydrate overlay warn-only/assignedTask 缺席时 squad fallback→publish_issue_created（钩子或缺省 {issue_id}）→capture analytics（agent: 前缀+classify_origin 三臂含未知 origin warn）→非 fire_at 走 maybe_enqueue_on_assign；
    - helpers：validate_labels 去重+resource_type='issue' 守卫（GetLabel 逐 id 免新增批查）；publish_attachments_changed 三级降级（reload 失败/workspace 失败各回退 rev0 无版本事件；先全量快照后附件增量的 revision 顺序语义）；maybeEnqueueOnAssign——backlog 停车场（effective 判定 MUL-6243）+RuntimeUnusable→note_runtime_unusable 可见拒绝（MUL-6164，仅此处因 assignment 无应答可读）+直接入队/squad fallback；agentAssigneeVerdict 仅 BLOCKED 拦截（离线机器照常排队）——acquire 失败镜像 Go 错误路径走默认裁决且 squad fallback 照常；enqueue_squad_leader_task head_sha 键控去重（TEN-356）；note_runtime_unusable——系统评论 author_id=零 UUID 有效值（列 NOT NULL，客户端按 author_type 分支）+comment:created 载荷仅七键无 parent/source 键与 agent 评论载荷刻意不同
    - will_enqueue_run 谓词：双侧 effective 归一化（MUL-6243 自定义 status 继承类目+MUL-6463 backlog 类目内改键不算离开）；assign 源（create/assignee 变更，backlog 即 None）与 status 源（prev=backlog 且现非 backlog/done/cancelled+self_loop 探针）选择；agent 臂 runtime/archived/can_access/self-suppress（仅 assign 非创建）/status 源 pending 守卫；squad 臂 leader readiness 用 Ready() 非 blocked、self-suppress 刻意不适用（squad 指派=有意群体移交）；has_pending_run fail-closed（错误读作 pending 防 preview 过度承诺）
    - 配套：agent_readiness 泛化 executor 参数使事务内可用（&Pool 调用点不受影响）；issue_status::resolve 去 Copy 约束
 — **clippy -D warnings/fmt 干净，368 测试全过不变**
 → [x] **empty_claim_cache.go(197) 全量落地** → src/empty_claim_cache.rs（Redis 负缓存+版本标签防竞态：mul:claim:runtime:{empty,version}:{id} 双键、3min 判定 TTL/24h 滑动版本 TTL/250ms 每调用超时三常量；current_version——GET+EXPIRE 滑动续期单超时包裹、nil 静默归 0 对齐 redis.Nil 分支；is_empty——MGET 元组解构、版本键缺失读作逻辑 0（新建 runtime 快路径可达性关键）、字符串相等判定；mark_empty——SET EX 3min 记录观测版本；bump——INCR+EXPIRE 原子管道、错误仅告警由 TTL 兜底；ConnectionManager clone 句柄 + cmd/pipe 局部绑定规避 E0716 临时借用；消费方 Option<EmptyClaimCache> 承载 Go nil-receiver 安全语义） — clippy/fmt 干净 368 过
 → [x] **channel_media_reconciler.go(389) 全量落地** → src/channel_media_reconciler.rs：意图台账对账器——七常量固定调度（15min settle/1min sweep/2min lease/50 limit/1min<<n backoff cap 1h/30s delete timeout 全部以 Duration 交 SQL 由 Postgres 时钟判定防漂移副本）；[15m,1h,6h,24h] 加宽墓碑重删表（废弃 PUT 迟到物化由后续 pass 收回）；MediaObjectDeleter trait 接缝（S3/LocalStorage 后续满足）；run 循环 tokio interval 消费首 tick 对齐 Go NewTicker 满间隔首拍+CancellationToken 停机；run_once 逐行 claim→settle 至上限+backlog/tombstones 指标；settle 引用复查在 claim 翻 deleting 之后（否定即终态非竞态快照）且墓碑 pass 也复查；墓碑被引用=不变量破坏 error 日志+tombstone_referenced 计数保对象不删；settle_deleted_object 存储删除在事务外持 lease 自限超时→成功后按 schedule 墓碑化（重删幂等日志区分）/耗尽清行、失败 release 走 backoff（attempt 饱和移位防下溢）last_error 落库；shutdown 各写点 cancel 早退交由 lease 过期回收 — **clippy -D warnings/fmt 干净，368 测试全过不变**
 → [x] **plugin_mcp_transport.go(301) 全量落地** → src/plugin_mcp_transport.rs(563 行)：discover 复用 cordy_remotemcp::discover（该模块此前已先行落地——MCP JSON-RPC initialize/version 协商/initialized 通知/tools/list、SSE data 帧提取、Go 兼容 canonical JSON；先前审计 grep 因 head 截断误判未移植，本轮核实纠正）；九函数全量：discover_mcp_hook_tools(net: scope fail-closed+mcp transport 守卫)/approve_mcp_hook_tools(按名发现校验+空名单=撤销批准+approved_at RFC3339/approved_by 零 UUID 省略)/agent_mcp_connections(enabled 过滤聚合)/mcp_connections_for 纯函数三条件(mcp transport+agent trigger+非空 approval)+credential_header 按 CONFIG_SECRET 字段型注入 Authorization/tool_set_digest_or_empty 失败安全降级/mcp_credential_headers(secretbox 解密失败降级空头)/decrypted_secret(secrets None→Unavailable 'CORDY_PLUGIN_SECRET_KEY is not configured')/approved_mcp_tools 按名索引/mcp_hook_credential 首对返回；PluginMCPApproval serde BTreeMap 有序键对齐 Go map marshal
 — **clippy -D warnings/fmt 干净；全 workspace 测试 482 全过（composio 未提交模块测试套件随 needless_borrow 修复首次纳入计数，368→482）**

 → [x] ⑨ plugin 系列(~2,700)
 → [x] ⑩ autopilot(1,941+396)：[x] **agent_ready.go(200) 全量前置** → src/agent_ready.rs（AgentAvailability 三态 Available/Waitable/Blocked——WAITING 是否为计划的核心区分、AgentVerdict{availability,reason,repair,detail}+ready/blocked、RuntimeRepair serde omitempty 对齐 daemon wire 格式、AgentReadiness archived→TargetUnavailable / 无 runtime→AgentRuntimeRequired / GetAgentRuntime ErrNoRows→硬错误供 fail-closed 方、runtimeVerdict 纯函数 online→Available、offline_reason envelope 解析（不可解析≠另一种裁决）not_executable→Blocked 携带 repair 其余→Waitable 带状态 detail、RuntimeUnusableNotice 双文案（有 repair 命令带 fence 语言 powershell/bash 无则通用重装指引））— 5 测试 + [x] **纯函数锚点全量** → src/autopilot.rs（AutopilotService{pool,bus,task_svc:Arc} Queries/TxStarter 折叠一个 pool、record_autopilot_rule_version MUL-4302 §7 config summary 序列化、is_run_complete completed/failed/skipped 直判+issue_created 看 issue 行+running 看 task 行 #4443、dispatch_fail_reason_code FailClosed 三变体→attribution_blocked 其余 internal_error、taskFailureReasonForAutopilotRun error→failure_reason→'task failed' 回退、formatAdmissionReason squad 前缀+MUL-1899 'at dispatch time' 后缀、ErrSquadArchived 独立哨兵、resolve_leader 四分支（agent/squad+archived fail-closed/unknown type 错误）返回 squadResolved 区分 fail-open/closed、squad_attribution、assignee_analytics best-effort leader 回退 squad id、autopilot_error_type 五桶表、actor_id 零值 UUID→system 映射注记、run_duration_ms 负值钳零（triggered_at NOT NULL 收敛 Go Valid 链）、resolve_trigger_timezone chrono-tz 解析校验三路回退 UTC+warn、formatAutopilotRunTimestamp/Date %Y-%m-%d %H:%M 对齐 Go layout、buildIssueDescription webhook payload 内联（event envelope→eventPayload pretty→整体 pretty→原样四级回退）prettify_json=MarshalIndent 两空格对齐、{{token}} 正则 \{\{\s*([^{}]*?)\s*\}\} OnceLock、interpolate 容忍花括号内空白与 validate 接受面一致、validate_issue_title_template 首个未知变量名进错误文案、SUPPORTED=[date]、quota 头部类型：AutopilotQuotaMetrics trait/QuotaExceededError 四事实字段/QuotaUsage policy-neutral Optionals/blocked_counts、QuotaPolicy 投影（allow dead_code 待 quota 方法族构造）、idempotency key v7 替代 v4 注记（opaque 单请求作用域契约不变）、valid_execution_source 四值白名单） — **clippy/fmt 干净，368 测试全过（362→363→368）**
 → [x] **方法族·子片A：终态迁移 + 同步监听 + capture/publish + skip 机制**（AutopilotService 加 quota_metrics seam 字段；UpdateAutopilotRunTerminalWithQuota 单查询三态——complete 携 result+consume=true、fail/skip 带 reason/code 不消费——run_from_terminal_row 映射回完整模型；settle_autopilot_quota ErrNoRows=已终态回放语义 Ok(false)、recover_partial 行数>0 判定、fail_runs_by_issue 保 create_issue 消费不可变；record_quota_decision seam；publishRunDone 三键 payload；capture 四件——issue_created 用 leader 作 agent_id 对齐 daemon 上报、run_started triggerSource 兼 cadence 代理、run_completed source 双用、run_failed reason 空→unknown+error_type 五桶+will_retry=false；ErrDispatchSkipped{reason,code} pub + DispatchError 双臂 Skipped/Service + handle_dispatch_skip（skip 改写失败容忍保留现态由 failure monitor 兜底、last_run_at 与 pre-flight skip 对齐 bump）、fail_run internal_error 兜底；SyncRunFromIssue origin 门控+effective 状态 done/in_review→completed、cancelled/blocked→'issue {status}' 审计留人类所选原 status MUL-6243；SyncRunFromTask error 空串仍覆盖粗标签的 Go Valid 语义保留；SyncRunFromLinkedIssueTask 单查双义+HasActiveTaskForIssue true→return 等最终尝试） — **clippy -D warnings/fmt 干净，368 测试全过不变**
 → [x] **方法族·子片B：quota 方法族**（EntitlementProvider trait seam 替身 Go entitlement.Provider——三态 Off/Observe/Enforce + GateDecision 投影；quota_policy 畸形策略 fail-open 且零配额表访问（Cloud 唯一区间构建权威语义保留）；create_run_with_quota 单事务五段：EnsurePeriod 锁区间行 → 幂等键查 reservation（reused 提交返回 / 孤儿 reserved 行释放后续走新预约路径防稳定键整期卡死 / 未命中走新建）→ would-block 且 enforce 时 IncrementBlocked+提交+记 blocked+返回 QuotaExceededError 四事实字段 → observe-only would-block 只进决策指标不落持久计数 → CreateReservation+IncrementReserved+注入 reservation id 建 run+提交记 admitted/would_block；QuotaAdmissionError 三臂 InvalidSource/Exceeded/Internal；quota_usage 无 period 行读作零计数+blocked_counts JSON 解码失败硬错；quota_enabled=seam 是否在位；reconcile_quota_reservations CAS 并发安全四分支（orphan 释放/completed 消费/failed·skipped 释放/manual·api 且无 issue·task 的部分态 recoverPartial），schedule/webhook 重试自管其恢复为防御性 no-op；insert_run helper + CreateAutopilotRunParams 镜像 Go sqlc params 减配额注入列）
 → [x] **方法族·子片C：准入 gate + 调用权限 + skipped run 落库**（ResolveLeaderError 增 NotFound{squad_resolved} 分型——三路硬 skip 判定输入：archived squad / squad 无法解析 / agent 迁移 096 无-FK 世界已硬删，瞬态 DB 错误仍 fail-open 留给下个调度 tick；should_skip_dispatch：无 assignee→TargetUnavailable skip、resolve_leader 归档/缺失硬 skip、readiness 加载故障 warn+fail-open、create_issue 执行模式对 Waitable 离线 runtime 放行（issue 服务端落库等本子上线，不可用 runtime 不放行避免注定失败的建单）、其余走 formatAdmissionReason 带 MUL-1899 后缀；MUL-3963/MUL-4525 调用门：manual run-now 门当前点击者、automation 回落创建者、无 admin bypass；can_member_invoke_agent owner 直通 + public_to 门 + workspace/member target 匹配，nil-UUID 早退防 nil-owner×nil-caller 假授予；can_creator_invoke_agent member 创建者须 workspace 成员、agent 创建者按 workspace 内部主体判定 workspace-broad；record_skipped_run 预检 skip 直接落库（刻意不经配额表）+ UpdateAutopilotRunSkipped best-effort 盖原因 + last_run_at bump 与 pre-flight 语义对齐 + run-done 广播）
 → [x] **方法族·子片D：dispatch 核心**（DispatchOutcome{run, reason_code} 承载成功/skip/复用三态——Go 三返回值折叠为 Result<Outcome>，schedule 配额拒绝折为 Ok(Some(QuotaExceeded)) 而 manual/webhook 保持硬错误（handler 切片可下转型 4xx）；dispatch_autopilot：admission skip→record_skipped_run（无 reason_code 参数与 Go 对齐）、initial_status 按 execution_mode 分流、reused 路径经 reason_code_from_wire 回读 wire 码；dispatch_autopilot_run 三分支——Skipped 走 handle_dispatch_skip、真失败 failRun+captureFailed+Err、未知 execution_mode 同构；dispatch_create_issue 单事务七步：resolve leader→模板插值/描述构建→GetAutopilotInWorkspace 刷新 project 绑定→recent-duplicate 守卫（AlreadyActive skip，issue_guard 两函数转具体 &mut PgConnection 签名——advisory xact lock 强制事务作用域+消除 Copy 泛型）→issue counter+NextTopPosition→CreateIssueWithOrigin（creator=resolved leader 使 activity/mentions 作者身份正确，人类配置者走 origin_type=autopilot）→模板订阅者同事务 fan-out（通知监听器首个事件即见完整集合）→UpdateAutopilotRunIssueCreated+settle consume→提交后 issue:created 广播（IssueToMapWithCategory 权威 category）+analytics+订阅者收件箱行（member-only 守卫+best-effort 不回滚 issue）；入队按 actor 分流——manual 走 WithHandoff 变体使归因 direct_human 落到点击者（MUL-4302 §4），automation 走普通路径由 autopilot-origin 归因 rule_owner，squad 先 admit_invoke 复检 fail-closed；dispatch_run_only：leader 缺失/归档→skip TargetUnavailable、readiness 未就绪→skip 带 verdict.reason、squad 调用门复检→InvocationNotAllowed skip、归因分流 manual direct_human / automation trigger_owner→fallback 链失败→AttributionBlocked skip、create_autopilot_task 含标题快照截断、UpdateAutopilotRunRunning warn-only、NotifyTaskEnqueued 补唤醒（绕过 TaskService.Enqueue* 的直接插入必须显式唤醒防 empty 缓存饿死）；全部 allow(dead_code) 待接线标注随核心落地移除）
 → [x] **方法族·子片E：dispatch 入口层（autopilot 域收官）**（dispatch_autopilot_public——schedule/webhook/api 无成员 actor 走 rule_owner 归因、key=source+随机幂等键、丢弃 reason code 与 Go 对齐；dispatch_autopilot_manual/_with_key——manual 是唯一向人类暴露逐 run 结果码的入口面、key=manual:{autopilot_id}:{caller_key} 保留调用方请求键防同请求重复预留/执行、Option<Uuid> actor 保留 Go 非法 UUID 即自动化派发的语义；admit_autopilot_webhook_delivery——delivery_id 必填校验、按 delivery 幂等查找命中即返（HTTP 200+run_id 契约）、skip 路径 record_skipped_run 失败回退并发恢复、create_run_with_quota key=webhook:{delivery_id} 后 capture started；recover_concurrent_webhook_admission——Go 按 23505 类型化判定改无条件 reload 尝试：仅复用真实存在行故语义等价，非竞态失败多一次廉价读且配额拒绝仍正确传播；dispatch_autopilot_for_webhook_delivery——admit→complete 短路（create_issue 有 issue 则 ensure_webhook_create_issue_task 补任务）→run_only 无 task_id 先 repair_autopilot_run_task_link→否则继续部分态 run 的 dispatch_autopilot_run（nil actor 丢码）；ensure_webhook_create_issue_task——ListTasksByIssue 非空即下游所有权已移交直接返回、issue effective 非 todo/in_progress 不补、squad 解析 leader 后走与原派发一致的入队路径；repair_autopilot_run_task_link——GetAutopilotTaskByRun 缺行返 None、UpdateAutopilotRunRunning 链接后终态任务走 SyncRunFromTask+GetAutopilotRun 重载、活跃任务 NotifyTaskEnqueued 补唤醒；dispatch_autopilot_for_plan——(trigger_id, planned_at) 经部分唯一索引幂等：complete 短路返回供 job 记 SUCCESS 不重复副作用、run_only 无 task_id 先 repair、partial warn 日志含 issue_set/task_set 后 recoverPartialAutopilotRun false→'changed concurrently; retry'、key=schedule:{trigger_id}:{RFC3339Nano}；Go 零时间守卫在 DateTime 类型下无对应物已注释） — **clippy -D warnings/fmt 干净，368 测试全过不变**
 → [x] ⑪ chat_quick_actions_generate 剩余 TaskService 方法（ForTask/Async/Supplement/eligible/in-flight/placeholder/buildPrompt + parse 族）
 → [ ] ⑫ channel_media_reconciler(389) + empty_claim_cache(197) + agent_ready(200，autopilot 前置) + runtime_unusable_notice(69，注意与 agent_ready::runtime_unusable_notice 同名族——Go 里就是两个文件各一份调用方) + squad_no_action(complete_task 已内联其查询消费) + plugin_mcp_transport(301，待查 remotemcp.Discover 就绪度)
  - [x] plugin.go (854) + plugin_hook.go (569) → **plugin 域全量移植（除 mcp transport 依赖 remotemcp.Discover 延后）**：
    - **前置修复**：rustls 双 crypto provider 歧义 panic——workspace rustls/tokio-rustls 改 default-features=false+ring feature（sqlx/reqwest/jsonwebtoken 全链 ring），三处 ClientConfig::builder 显式 builder_with_provider(ring)（remotemcp client.rs/devorigin.rs、email.rs）；aws-lc-rs 及其 cmake 构建链出图
    - plugin.go → src/plugin.rs 补全 Slice2：PluginService 结构体（pool+secrets(SecretBox)+local_dir/dev_origins env 装配 new_from_env、host capabilities、deployment_key；Go Queries/TxStarter 二字段折叠为一个 sqlx pool——executor 泛型查询 + pool.begin()）、PluginPreview serde 对齐 Go json tag（installed bool omitempty 用 Not::not、version/scopes 空值省略）、fetch_manifest 三路分流（local:/dev-origin/remote SSRF guard）、read_local_manifest/read_local_file（单段目录名校验+ParentDir 逐级弹出防逃逸，等价 filepath.Clean+prefix 检查且更严——`..` 弹出根目录即拒）、preview_plugin（capabilities 不兼容→Incompatible+missing 列表）、install_plugin 新装/升级双路径（升级事务内 orphaned secrets 清除→manifest 快照更新→skill 资源重跑；canonical bytes 经 canonical_json_value 包 Value 绑定）、set_config（secret/plain 分流、空串清密文、merge 防表单漏提交静默清除、无 Secrets 时 fail-closed 拒绝 secret 写入）、configured_secret_keys/set_enabled/uninstall（五删除单事务：storage→secrets→invocations→skills→installation）/installation_for_workspace（workspace 绑定防跨工作区操作）
    - plugin_skill.go → src/plugin_skill.rs：install_skill_resources_in_tx 事务内 prune-first（rename 先释放旧名防 unique 冲突）→逐资源 fetch（相对 manifest URL JoinPath("../",entry) 解析、256KB 上限、空白拒绝）→frontmatter description 缺省回退 "Provided by the {name} Plugin."→upsert（23505→Conflict "a skill named %q already exists"）；**frontmatter.go 移植**：--- 围栏解析容忍 \r\n、serde_yaml 通用 map 解码按 key 独立 coerce（结构化值不丢弃兄弟键）、block scalar 尾换行 trim 归一（MUL-5645）
    - plugin_token.go → src/plugin_token.rs：mpi_ 安装令牌（SHA-256 hash 存储、明文仅返回一次、revoke 置 NULL）、mpc_ 回调令牌 CallbackTokens（Mutex<HashMap> 内存态、5 分钟 TTL、sweepLocked retain 过期清理、多调用有效至过期——单次即焚被实测否决：handler 读后写需两调用）、HookActor/CallbackGrant 类型
    - plugin_hook.go → src/plugin_hook.rs：invoke_hook 引擎（disabled/trigger 白名单/http-only 三守卫→check_hook_rate（DB 计数失败映射 0 遥测不致断）→callHookEndpoint→record_invocation best-effort）；call_hook_endpoint 目的地校验双路（dev-origin 直连无重定向 reqwest / 公网 pinned connector dial 时重解析拒私有 IP），net_domains 空 fail-closed、hookRequestBody wire 格式（version=1、issue_id host 侧解析下发、config 非 secret 过滤——对照 MANIFEST 而非存储形态、callback_token 发放后调用结束即 revoke 无论成败）、响应 1MB 上限+非 JSON 拒绝、hook_breaker_open（5 失败/5min 熔断事件投递）
    - plugin_event_dispatch.go + plugin_event_bridge.go → src/plugin_event_dispatch.rs：PluginEventDispatcher（tokio mpsc 512 深度有界队列、4 worker tokio::Mutex 串行 claim、CancellationToken 优雅停机、dropped AtomicI64 背压计数）；dispatch 仅入队不解析（安装查询与 issue id 提取都在 worker 上、flag-off 零成本）；run 每 delivery 实时读 plugins_v1 flag（关掉立即停外呼而非下次重启）；deliver 熔断跳过+3 次 2s 退避重试+refused 即弃；sweep_invocations 每小时清 7 天前 invocation 行（首扫等 tick——Go 曾在 router 测试因未开池的 Queries 立即清扫而 panic）；bridge 订阅五个 protocol 事件映射七个插件事件（issue:updated 携 status_changed=true 派生 issue_status_changed）
    - plugin_action.go → src/plugin_action.rs：authorize_plugin_action（存在性+enabled+scope 三查，资源级权限留给常规 loader 不复制权限规则）、PluginActionCaller{issue_scope 单一 issue 限定}、build_plugin_context（config 只含非 secret 值、user 缺省=plugin 自身 actor）、issue_identifier 纯函数
    - plugin_storage.go → src/plugin_storage.rs：两 scope（workspace/user）resolve、纯函数 enforce_storage_quota（value 100KB/key 数 1000/总量 5MB 三限各自消息、usage 排除写入键防替换误判）、CRUD 四方法（delete 0 行→NotFound）
    - plugin_agent_tools.go → src/plugin_agent_tools.rs：plugin_tool_name 可注入命名（末段可读名+完整 key SHA-256 前 6 位 digest 防 a.b/a-b 折叠——测试捕获的非单射缺陷、__ 分隔符因 hook key 模式保证不出现）、agent_hook_tools 按 name 排序稳定输出（防 provider prompt cache churn）、invoke_agent_hook（actor=agent 本人授权）
    - gen_db_port 缺陷修复：upsert_plugin_secret ciphertext 参数 BYTEA 列误绑 serde_json::Value → &[u8]（生成器无法从 sqlc param 看列类型，手改模式确立）
    - cordy-db queries/plugin 33 查询全数复用；**新增 19 测试，cordy-service 共 142 测试，全 workspace fmt/clippy 干净、362 测试全过**
    - **延后**：plugin_mcp_transport.go (301) 依赖 cordy-remotemcp.Discover（MCP JSON-RPC/SSE 协议层，remotemcp crate 已标注 deferred to plugin-series slice）
  - [x] autopilot.go (1,941) + autopilot_quota.go (396)——全量落地：方法族五子片(A 终态+同步+capture+skip / B quota 全族 / C 准入 gate+调用权限+skipped run / D dispatch 核心 / E dispatch 入口层)
  - [x] ⑧ issue.go (765) + issue_trigger.go (218)——新模块 issue_service.rs 全量落地（含 runtime_unusable_notice.go）
  - [x] agent_ready.go (200) —— 已作为 autopilot 前置落地
  - [x] empty_claim_cache.go (197)、runtime_unusable_notice.go (69，随 issue_service 落地)
  - [x] channel_media_reconciler.go (389)
  - [x] plugin_mcp_transport.go (301)——Discover 已在 cordy-remotemcp 先行落地，阻塞解除

### S6 收尾审计（本轮逐文件核对）

| Go 文件 | Rust 落点 | 状态 |
|---|---|---|
| task.go (6710) | task_service/terminal/recovery/notify/helpers + quick_actions 服务方法 | ✅ Slice1-⑥ |
| issue.go+issue_trigger.go | issue_service.rs | ✅ |
| autopilot.go+quota.go | autopilot.rs（五子片） | ✅ |
| plugin.go 系列 ×9 | plugin*.rs ×8 | ✅（mcp_transport 阻塞 Discover）|
| chat_quick_actions(_generate).go | chat_quick_actions.rs + task_quick_actions.rs | ✅ |
| cron/email/builtin_agents/builtin_skills | 同名 .rs | ✅ |
| agent_ready/runtime_unusable_notice/squad_no_action | agent_ready.rs / issue_service.rs / task_terminal 内联 | ✅ |
| empty_claim_cache.go | empty_claim_cache.rs | ✅ 本轮 |
| channel_media_reconciler.go | channel_media_reconciler.rs | ✅ 本轮收官 |
| plugin_mcp_transport.go | plugin_mcp_transport.rs | ✅ 本轮收官 |
| 叶子包 attribution/dispatch/featureflags/issueguard/issueposition/issuestatus/redact/skillbundle/taskfailure/dbid | 同名 .rs / cordy-db | ✅ |

**结论：S6 全部 26 文件清零收官。全 workspace 测试 482 全过（含 composio 未提交模块测试首次纳入），clippy -D warnings/fmt 干净。**

### S7 执行记录（接手会话 20260821 晚）

#### 接手修复（会话 20260822，Carson 遗留现场清理）

- [x] **Slice4a 去重与修正**（cordy-service/task_service.rs）：Carson 补丁把 CompleteTask 族整体重复移植了一遍——task_terminal.rs 已有提交版（L3695-5131 全量），删除重复的 complete_task/write_chat_completion_outcome/chat_quick_actions_eligible/observe_chat_output_local_path/CHAT_NO_RESPONSE_FALLBACK；保留四个真正新增的生命周期方法并逐条对齐 Go 源：
  - start_task 合并两份（已提交版缺 ReconcileAgentStatus，Carson 版有；escalation 取消调用点 Carson 版错传 task.escalation_for_task_id，Go 是无条件传 task.ID——已纠正）；无行错误映射 Internal("start task: no row written") 对齐 Go fmt.Errorf 包装语义
  - cancel_deferred_escalations_for_issue_agent 修正查询调用错误（原调单参数 cancel_deferred_escalations_for_task 且传 issue_id；改用 cordy-db 已生成的双参数查询 cancel_deferred_escalations_for_issue_agent）
  - mark_task_waiting_local_directory 的 PREPARE_LEASE_SECS 未定义 → PREPARE_LEASE_DURATION.as_secs_f64()（Go 同一常量 .Seconds()）
  - extend_task_prepare_lease 签名核实无误
- [x] **dingtalk 编译修复**：decode_dingtalk_raw 导入路径（inbound→resolvers）；fetch_bytes 循环内 Ok(...) 改 return（loop 块值≠函数返回值，Go return 直译坑）；ingest_one 双索引拆分——枚举序号 index 用于 object key/filename（dingtalk-image-{i+1}）、资源自带 inline_index 用于 MediaRef（Go 测试 PartialFailurePreservesOriginalInlineIndex 证明两者必须独立）；inst 引用逃逸 spawn → clone 进任务
- [x] **wecom/dingtalk/service clippy+fmt 清零**：deprecated from_slice allow 注明原因、is_multiple_of/range_contains/repeat_n/needless_borrow/match_single_binding/explicit_counter_loop/question_mark/doc 缩进 ×6/too_many_arguments(allow)/unused imports ×8
- **验证基线：workspace（除进行中的 cordy-daemon）clippy --all-targets -D warnings 0 错误、cargo fmt 干净、737 测试全过（482→737，含三 crate 新增套件）**

### S9 并行车线（20260822 启动）

防冲突地基已铺：cordy-daemon lib.rs 预声明全部模块 + 空桩文件 + Cargo.toml 依赖超集预置（futures-util/walkdir/tempfile/libc 进 workspace 表）——各车道只填自己的文件。

| 车道 | 范围 | 文件所有权 | 状态 |
|---|---|---|---|
| W | daemonws hub.go(920)+notifier.go(130) | src/hub.rs, src/notifier.rs | ✅ **完成**（hub.rs 2,336 行+notifier.rs 590 行，符号映射头+37 测试；metrics.go 内嵌 hub.rs 因 lib.rs 模块冻结；ULID→UUIDv4 已注记；socket 泵留 S8 axum 层） |
| E1a | execenv 地基四件(execenv/context/isolation/local_worktree)≈3.4k 行 + 修复 git.rs 三错 | src/execenv/{execenv,context,isolation,local_worktree}.rs + git.rs 编译修复 | 🔄 运行中（execenv.rs 65KB 在写） |
| E1b | codex 全家(codex_home/sandbox/memory/shell_env/multi_agent/user_skills/skill_strip/cursor_mcp) ≈3.4k 行 | src/execenv/{codex_*,cursor_mcp}.rs | ⏳ 排队等 E1a |
| R2 | repocache/cache.go(1811)+gc.go(1509)+processtree 内联 | src/repocache.rs, src/gc.rs | ✅ **完成**（repocache.rs 2,546 行 + gc.rs 2,395 行含 processtree 内联；Cache 前台优先锁、WithRepoMaintenance 拆为 try_begin_maintenance+MaintenanceGuard（drop 解锁，对应 Go defer Unlock）、GcHost trait 接缝、S9-integration seam stand-in 标注；clippy -D warnings 0、73 测试全过。execenv/git.rs 三处悬挂括号、GcMeta serde 缺 default、codex_user_skills 缺失等跨车道编译阻塞一并修复；E1b 空桩文件以 TEMPORARY stand-in 补位待其替换） |

> 教训：前两轮车道卡死源于"先批量读完所有源再动笔"的长读阶段——重发时改为逐文件读写交替的早写节奏后恢复健康。

后续排队：A 车道(client/wsrpc/config/types/health/wakeup/diskusage/identity/poisoned/reconcile/canonical_path/thread_name/helpers ≈5.5k) → B 车道(daemon.go 核心 8814 + agents_probe/refresh/local_skills/skill_cache/prompt/artifact_matcher/claude_plugins/slash_skill) → D 车道(auto_update/local_directory/openclaw_runtime_config/plugin_hook_mcp/remote_mcp_broker/runtime_mcp)

- [x] **S7-c slack**(3,877)：full domain（提交 8d41cb0，另一并行会话完成）

- [x] **S7-a0** cordy-channel 地基 crate + channelmedia → cordy-util（提交 61405ef）
- [x] **S7-a1** engine 叶子件：LeaseStore trait+RedisLeaseStore（Lua 字节级对齐）、PendingBatcher 防抖器、/issue+/new 解析器、provenance（5dbf5dd）
- [x] **S7-a2** Supervisor：lease CAS 生命周期、凭证轮换重启、backoff+jitter、rotation 等待（0ed05c6）
- [x] **S7-a3a/b** ResolverSet 接缝层（9 个 trait）+ Router 8 段入站管线全量（e8668f8/d0b26b1）
- [x] **S7-a4** session_media 纯函数半区：内联占位符替换、/issue 描述组合（86529aa）
- [x] **S7-a5** session.rs DB 事务体：ensure_session（UNIQUE 竞态仲裁+workspace FOR KEY SHARE）、append_user_message（in-tx dedup Mark+ClaimLost 回滚）、bind_media_refs（intent claim 原子性）——create_attachment 可空 FK 手改 Option\<Uuid\>（2ede740，420 测试）
- [x] **S7-b** DbMediaIntentLedger 适配器：state-guarded upsert→Ok(false) 不复活 reconciler 持有的 key（892ca11）。engine 包非测试代码全部清零
- [x] **S7-k** remotemcp.Discover（MCP JSON-RPC/SSE 协议层：initialize/initialized/tools-list 握手、SSE data 帧解析、canonicalJSON、ToolSetDigest）+ plugin_mcp_transport 服务层全量（approval 按 name+digest pin、mcpConnectionsFor 三条件判定测试化、secretbox 凭证头）（0e9ba03，435 测试）
- [x] **S7-h ghsnapshot**：client.rs+snapshot.rs 落地（Go 1,115 行→Rust 911 行，36 函数中核心刷新管线覆盖）
- [x] **S7-i composio**：service/sdk/dispatch/state 四模块（Go 1,050 行→Rust 2,889 行；begin_connect/complete_callback/list_connections/disconnect/create_mcp_session/list_toolkits 全量+SDK mock 测试化）
- [x] **S7-j vcs**：vcs.rs+forgejo.rs+gitlab.rs（Go 649 行→Rust 1,233 行；Provider trait+事件解析+签名验证+token 校验三平台全量）
- [x] **S7-d telegram**(3,470→Rust ~1,900)：api.rs（Bot API envelope/50s long-poll/409 Conflict/429 Retry-After 单次重试）+ config.rs（加密 token 解码、bot id 校验）+ markdown.rs（code/link 占位符先于 escape、粗斜删/标题/列表/fence、Go html.EscapeString 五字符对齐）+ inbound.rs（UTF-16 实体偏移、@mention 边界 @botfan 不误伤、quoted-human 增强、/new fresh、媒体分类表）+ sender.rs（4096 UTF-16 分块 newline 偏好、HTML 失败回退纯文本、首块才引用）+ resolvers.rs（bot_id→installation、binding+membership 身份、两阶段 dedup、topic 分会话路由 ResolverSet 装配）（8de1c4a+8c49f1e，25 测试）。**outbound.go（1,117 行流式编辑/终端回复队列/backoff 日历）未移植**——随 S8 handler 接线切片落地
- [ ] **S7-c slack**(3,877)：history/media_ingest/resolvers——空脚手架（另一并行会话认领）
- [ ] **S7-e dingtalk**(3,918)：resolvers——空脚手架（另一并行会话认领）
- [ ] **S7-f lark**(10,060 最大域)：http_client/registration/inbound_enricher/outbound/ws_connector/media_ingest/channel_store/outcome_replier——空脚手架（另一并行会话认领）
- [ ] **S7-g wecom**(7,525)：wecom_channel/ws_frame/outbound_media/installation/media_ingest/markdown/media_download/media_upload——空脚手架（另一并行会话认领）

### S7 九域盘点表

| 域 | Go 行数 | Rust crate（行数） | 状态 |
|---|---|---|---|
| channel(+engine) | 4,329 | cordy-channel(1,146)+channel-engine(5,077) | ✅ 引擎清零（S7-a0..b） |
| composio | 1,050 | cordy-composio(2,889) | ✅ 四模块 |
| vcs | 649 | cordy-vcs(1,233) | ✅ 三平台 |
| ghsnapshot | 1,115 | cordy-ghsnapshot(911) | 🟡 client+snapshot 在位；**Manager 编排层(~430 行)未移植**——refresh.go 的 Enqueue/worker/process/rateLimitPause/applySnapshot/scheduleRetry/sweepLoop 全链 |
| lark | 10,060 | cordy-lark(3) 空脚手架 | ⬜ 最大单域 |
| wecom | 7,525 | cordy-wecom(3) 空脚手架 | ⬜ |
| dingtalk | 3,918 | cordy-dingtalk(3) 空脚手架 | ⬜ |
| slack | 3,877 | cordy-slack(3) 空脚手架 | ⬜ |
| telegram | 3,470 | cordy-telegram(3) 空脚手架 | ⬜ 最小起步点 |

推进序（空脚手架小→大）：telegram → slack → dingtalk → wecom → lark

> ⏳ S9 daemon agent 排队超时（并发上限），待 S7 批次完成后重试

- [ ] **S8. handler + router**：475 条路由分域移植
- [ ] **S9. daemon + daemonws**
- [ ] **S10. CLI bins**：cordy/migrate/backfill×3
- [ ] **S11. 测试移植**：807 个测试文件按域同步移植
- [ ] **S12. 切换演练**：双跑对比 → 流量切换 → Go 目录归档

## 五、本次会话产出

- [x] Codemap + 计划文档（本文档）
- [x] S1 脚手架可编译（cargo build/clippy/test 全绿）
- [x] 垂直切片：config 加载 + sqlx pool(lazy) + /healthz + /readyz
- [x] 冒烟测试：服务启动、healthz 200、DB 宕机时 readyz 503 + 错误日志

### 已建立的模式（后续步骤遵循）
- **Workspace lints**：clippy all + unwrap_used/expect_used = warn，CI 用 `-D warnings`
- **配置**：env 变量名与 Go 完全一致（PORT/DATABASE_URL/REDIS_URL），TOML 为本地开发附加层
- **DB pool lazy 连接**：进程先起、readyz 报告 DB 状态（K8s readiness 语义）
- **测试触碰 env 必须串行**：`ENV_LOCK: Mutex<()>` 模式
- **错误分类**：`cordy_util::Error`（NotFound/Unauthorized/Forbidden/Invalid/Conflict/Internal），HTTP 映射在 handler 层统一做

## 六、Review

### 环境问题记录
1. `~/.config/opencode/opencode.json` 曾把默认模型指向已禁用的 `taokc` provider → 已改为 `opencode/x-preview-f-free`，重启 opencode 后子代理可用
2. `~/.cargo/bin/cargo` 是损坏的 shim（bash 脚本直接透传参数给 rustup）→ 本会话用 `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo` 绕过；建议用户修复或删除该 shim
3. 后台 task() 因模型解析失败不可用 → 全程直接探索
