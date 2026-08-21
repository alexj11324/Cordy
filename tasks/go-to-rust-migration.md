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
- [ ] **S6. service 层**：按域逐个移植（email/cron/issue/plugin...）——进行中
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
  - 修复中断会话遗留：cordy-analytics 缺 serde 依赖连锁编译错、PostHogConfig 未导入路径、空标签切片类型标注 ×6、events 测试断言与 Go withCoreProperties 空值省略语义矛盾（改断言非改实现）、record_event 配对测试 signup_source "x" 应归一为 "twitter" 桶、clippy too_many_arguments ×2 — **全 workspace 274 测试全过，clippy -D warnings 干净，fmt 已格式化** → [x] **④ task.go Slice1(1-975) → cordy-service/src/task_service.rs**：TaskService 结构体（pool+bus+analytics/metrics/wakeup/feature_flags/composio/quick_actions 可选接缝；Go 死字段 Hub 省略、quickActionsInFlight/Running 并发门、analyticsContext LRU 4096）；ErrAttributionFailClosed/ErrDuplicatePendingTask 哨兵 + is_duplicate_pending_task_err（23505 双索引名）+ pending_slot_taken_err 双形状匹配；归因族全量（buildCommentTriggerSummary MUL-4252 workspace 范围、resolveOriginatorFromTriggerComment/attributionFromTriggerComment/attributionFromComment 走 source_task_id→parent 链、AttributionForMergedComment isMention→delegation 标签、attributionForIssueTask actor>comment(跳过 system)>autopilot origin→triggerOwner>DirectFacts(origin 继承 quick_create/agent_create)、applyAttributionFallback fail-closed 三拒绝路径、ruleOwner/triggerOwner 自由函数）；buildRuntimeMCPOverlay feature-flag 门控+失败软降级；parseQuickCreateContext 无链接前置校验；taskAnalyticsContext 缓存优先+runtime→agent 回退+issue/chat/run 四路覆盖+quick_create 终写；captureTask* 七个 metrics 助手含 cancel 的 mat_ token 撤销(MUL-2600)；durationSeconds 反向钳零/taskErrorType 桶表 — 7 新测试（fixture 手写 52 字段结构体字面量因 models 无 Deserialize），**全 workspace 281 测试全过 clippy/fmt 干净** → [x] **⑤ Slice2(976-2186 入队族)**：enqueueIssueTaskWithCommentPlan（assignee/archived/runtime 三守卫、attribution→fallback→overlay→createParams 管线、fireAt 分流 CreateDeferredChannelIssueTask/CreateAgentTask、queued 广播先于 daemon 唤醒的 observe-order）；EnqueueTaskForMention/ThreadParent/SquadLeader(+Handoff)（delegation 标签、is_leader_task+squad_id、23505 双索引名→ErrDuplicatePendingTask 裸哨兵 #5914）；EnqueueDeferredAssigneeFallback（创建时盖归因章防 NULL-source bypass MUL-4302 §2）；EnqueueQuickCreateTask（context JSONB payload、direct_human 无证据对特例）；hydrateDeferredChannelIssueTaskOverlay（expected_originator 条件更新防 merge 竞态）；PromoteChannelChatTasksIfMediaReady/PromoteDeferredChannelIssueTask（ErrNoRows 幂等）；CancelTasksForIssue（MUL-4465 仅限 issue 生命周期清理）；事件发布族 taskEvent/broadcastTaskDispatch（context JSONB 展开+chat_session_id 路由键）/broadcastTaskFailedEvent（retry_pending 抑制 error 字段+redact）；ResolveTaskWorkspaceID 四级回退含 quick-create context；NotifyTask* 族（bump-before-wakeup 顺序注释保留）；EnqueueChatTask（FOR NO KEY UPDATE 锁序 chat_session→binding 防 ABBA、pending_fresh 提升 forceFreshSession、media deadline 读后 seal 再重推导 defer）；SendDirectChatMessage（overlay+归因在事务外解析、锁内重读 session+agent、HasPendingChatTurnForSession 位置语义、chat_input_task_id 自持有、AdoptOrphanOnboardingKickoff 先于用户消息、附件绑定、touch）；OpenMikaOnboardingChat（kickoff+opening 双行事务、ChatSessionHasUserMessage 防重）；RegenerateChatQuickActions（latest assistant turn 校验+stale 拒绝+busy 双闸）— TaskServiceError 扩至 14 变体（产品化哨兵与基建错误分层），anyhow→sqlx downcast 桥接生成查询层 — **全 workspace 281 测试全过 clippy -D warnings/fmt 干净** → [x] **⑥ Slice3(2187-3694 取消+认领)**：CancelTasksForIssue 补 distinctAgentIDs 去重 reconcile（D#3319）；CancelTasksForAgent/ByTriggerComment；BroadcastCancelledTasks（workspaceID 由调用方传入防已删行解析失败）；CancelTask/ByUser/WithReason→CancelTaskWithResult（SanitizeTextForPostgres 防 NUL 回滚 GH#7098、user_initiated 禁带 reason、QueuedOnly CAS、ErrNoRows 幂等返回现有行、chat_session→task 全局锁序）；CancelQueuedChatTasks（session FOR UPDATE→agent 锁序）；settleQueuedChatInput（channel_ingested→"Stopped." 行 vs 删除输入+edit 分离附件+draft restore）；finalizeCancelledChatMessage（空 transcript 三分支：channel 不可恢复/started+支持客户端→MarkChatFinalizeDeferred 延迟判定 #5219/否则 detach→delete→restore）；FinalizeDeferredCancelledChat（锁内原子 claim marker 防双 finalize、session gone 容忍、restored/stopped 双 outcome 广播 chat:cancel_finalized）；RebroadcastCancelledTask（幂等重播）；ReconcileAgentStatus/publishAgentStatus；claimTask（FOR UPDATE agent→容量检查→ClaimAgentTask prepare lease→direct-chat reanchor 兼容回退→300ms 慢日志）；ClaimTaskForRuntime（promote→stale reclaim 先于 empty cache→候选列表→按 agent 去重尝试）；ClaimTasksForRuntimes 批量版（MUL-4257 六步语义+部分成功返回防双 claim）；cancelSupersededDeferredRetries（23505 容忍跳过一个 tick）；PromoteDueDeferredTasksForRuntime；RequeueTaskAfterClaimFailure（CAS dispatched_at）；FinalizeTaskClaim（token+daemon token 过期清理+delivered comment ids 回执单事务）；StartTask（含 cancelDeferredEscalationsForTask）— **全 workspace 281 测试全过 clippy -D warnings/fmt 干净** → [ ] ⑦ Slice4a(3695-5131 终态+重试) + 4b(5132-6710 委托恢复+通知/映射) → [ ] ⑧ issue.go(765)+issue_trigger.go(218) → [ ] ⑨ plugin 系列(~2,700) → [ ] ⑩ autopilot(1,941+396，含 TxStarter 定义) → [ ] ⑪ chat_quick_actions_generate 剩余 TaskService 方法+channel_media_reconciler(389)+empty_claim_cache+其余小文件
  - [ ] plugin.go (854) + plugin_hook.go (569)
  - [ ] autopilot.go (1,941) + autopilot_quota.go (396)
  - [ ] channel_media_reconciler.go (389)、empty_claim_cache、其余小文件
- [ ] **S7. integrations**：9 个子集成逐个移植
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
