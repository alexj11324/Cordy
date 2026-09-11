# PR #810 交接：六平台消息连接（托管 / 自部署 / 禁用）

## 当前状态

- PR：<https://github.com/alexj11324/Cordy/pull/810>
- 分支：`claude/messaging-platform-e2e`，基于 `main` 的 `27c1b051`（PR #809 合并点）
- 工作树：`/Users/alexjiang/.cache/cordy-messaging-live-20260910`
- 旧的 Codex 工作树 `/Users/alexjiang/.cache/cordy-messaging-continue-20260910`（分支 `codex/post-809-workflow-fixes`）还留着大量与消息无关的未提交改动（Agents / Issue / Projects / Profile），不要整体提交或覆盖。
- 主检出 `/Users/alexjiang/Desktop/vibe/Cordy` 里的 `projects-page.tsx`、`.omo/`、`vendor/` 是用户自己的改动，不要动。

提交顺序：

| 提交 | 内容 |
| --- | --- |
| `2690ecf1` | CLI `issue status` 带上审核证据参数（独立事项，已验证） |
| `a6fdcaa5` | 部署网关在部署时从 GSM 读取 5 把新平台加密 key，不落盘，Slack key 不变 |
| `e15495a7` | 部署归属决定配置权限：托管可在 App 配置，自部署只能服务器配置，`disabled` 是总开关 |
| `b777ed30` | 六平台前端入口、自部署 bootstrap、微信服务器扫码、文档与脚本 |
| `4024d4f2` | 清理 `b777ed30` 从脏工作树带进来的无关改动（见"已修复的问题"） |
| `6fcc567a` | Agent 集成页测试补上托管策略前提，并覆盖自部署 / 禁用下隐藏入口 |

## 设计：谁来配置平台

模式只由部署归属决定（`server/internal/handler/config.go` 的 `messagingCapabilitiesFromEnv`）：

| 条件 | `mode` | App 能否配置 | 行为 |
| --- | --- | --- | --- |
| `ORVILO_APP_URL` 是 `orvilo.aspectlylabs.com` 或 `staging.aspectlylabs.com` | `managed` | 能 | 用户在 设置 → 集成 里连接 |
| 其他公网 HTTPS 地址 | `server_configured` | 不能（写接口 403 `server_managed_integration`） | 运维用环境变量 bootstrap 或 `orvilo-messaging weixin-auth` 配置，App 只显示服务器配置指南 |
| `ORVILO_MESSAGING_MODE=disabled`，或非公网地址（localhost、内网 IP、`.local`） | `disabled` | 不能（503 `messaging_disabled`） | 六个 adapter 都不启动 |

注意：`ORVILO_MESSAGING_MODE` 只剩 `disabled` 这一个值有意义。旧的 `managed` / `server_configured` 取值会被忽略，自部署无法把自己伪装成托管。

每个平台还需要自己的加密 key（`ORVILO_{LARK,SLACK,DINGTALK,WECOM,TELEGRAM,WEIXIN}_SECRET_KEY`，32 字节 base64），缺哪把，哪个平台就显示未启用。

## 托管用户怎么连接（生产上线后）

入口：设置 → 集成（工作区连接），或某个 Agent 的「集成」页（Agent 专属连接）。

| 平台 | 需要准备 | 连接方式 |
| --- | --- | --- |
| 飞书 | 飞书手机 App | 点连接 → 用飞书扫码 → 按提示创建个人 Agent 并授权（RFC 8628 device flow） |
| 微信 | 微信手机 App | 点连接 → 用微信扫 iLink 二维码；只支持文字私聊 |
| Slack | 一个开启 Socket Mode 的 Slack App（清单见 `deploy/messaging/slack-app-manifest.yaml`） | 托管 OAuth 还没配 client 凭据，所以 UI 会自动回退到"自带 App"：粘贴 `xoxb-` bot token 和 `xapp-` app token |
| 钉钉 | 企业内部应用 + Stream 模式机器人 | 粘贴 AppKey（client id）和 AppSecret（client secret） |
| 企业微信 | 智能机器人，开启长连接 | 粘贴 Bot ID 和 Secret，可选填机器人名称（群里 @ 识别用） |
| Telegram | @BotFather 创建的 bot | 粘贴 bot token |

连接后的消息流程：

1. 在平台里第一次给 bot 发消息，bot 会回复一张绑定卡片，链接指向 `https://orvilo.aspectlylabs.com/.../bind?token=...`。
2. 在已登录 Orvilo 的浏览器里打开这个链接，平台账号就和 Orvilo 账号绑定了（一次性链接）。
3. 之后的消息会进入 Agent 对话，Agent 的回复发回平台。
   - 工作区级连接：默认交给你可调用的第一个 Agent；在聊天里发 `/agents` 查看列表，发 `/agents 名字` 切换；没有可用 Agent 时 bot 会直接回复提示。
   - Agent 级连接：固定交给那个 Agent。
4. Agent 需要一个在线的 runtime（本机 daemon 或云端），否则任务会一直排队。

## 生产上线步骤

1. **先换网关**，这样合并后那次部署就会注入新 key：

   ```bash
   scp deploy/origin/production_deploy.py oracle:/tmp/orvilo-production-deploy.py
   ssh oracle 'sudo install -o root -g root -m 0755 /tmp/orvilo-production-deploy.py /usr/local/bin/orvilo-production-deploy && rm /tmp/orvilo-production-deploy.py'
   ```

   网关读取的 5 个 GSM secret：`orvilo-{lark,dingtalk,wecom,telegram,weixin}-secret-key`（项目 `general-secrets-store`，version 1）。任何一个读取失败，都会在改动容器之前中止部署，不会半途而废。
2. 等 PR CI 全绿后合并到 `main`，`Aspectlylabs production` 工作流会构建镜像并通过网关部署。
3. 验证：

   ```bash
   curl -s https://api.aspectlylabs.com/health
   curl -s https://api.aspectlylabs.com/api/config | jq .messaging
   ```

   期望：`commit` 是合并提交，`mode` 为 `managed`，`setupWritable` 为 `true`，六个平台都是 `enabled: true`。
4. 桌面端：已安装的正式版要等下一个 `v*` 发版才会拿到新 UI。发版前可以用 Actions → "Preview desktop build" 为分支打签名、公证过的 DMG；打包出来的 App 默认连 `api.aspectlylabs.com`。Web 端在部署后立即生效。
5. 回滚：用上一个健康的 `main` SHA 重新触发生产部署；如果要临时关掉所有平台，在生产环境加上 `ORVILO_MESSAGING_MODE=disabled` 后重新部署。

## 已验证的证据

分支 HEAD 上跑过：

- Go：`cmd/messaging`、`cmd/server`、`internal/messagingbootstrap`、`internal/handler`（完整）、`internal/integrations/{lark,slack,dingtalk,wecom,telegram,weixin,channel,channel/engine}` 全部通过
- 前端：`packages/views` 全量 468 个文件 / 5149 个用例（`TZ=UTC`，与 CI 一致）；views / core `tsc --noEmit` 通过
- 自部署脚本：`messaging-self-host-check.test.sh`、`messaging-self-host-setup.test.sh`、`selfhost-config.test.sh`
- 部署网关：`deploy/origin/production_deploy_test.py` 31 个
- CI：除 `frontend-views-test (2/2)`（已由 `6fcc567a` 修复）外全部通过

在本地真实后端上（`ORVILO_APP_URL=https://orvilo.aspectlylabs.com`，六把 key）：

- `managed` 模式：`/api/config` 返回 `managed`，`setupWritable=true`，六个平台都已启用
- 六个平台的官方 API 真实可达：
  - 飞书 device flow 返回 `open.feishu.cn` 授权链接；微信返回 `liteapp.weixin.qq.com` 二维码
  - Telegram / 钉钉 / 企业微信 / Slack 的无效凭据被各自平台拒绝，界面给出可读错误
- Slack 自带 App 安装后 Socket Mode 达到 `healthy`；断开后变为 `revoked` / `offline`
- `server_configured` 模式：Slack 通过 env + GSM bootstrap 真实连通；安装、扫码、断开等 App 写接口全部返回 403
- `disabled` 模式：六个 adapter 都未启动，写接口返回 503，微信列表为空且不可安装

## 还没做完的

- **各平台真实往返**（平台发消息 → Agent 回复到平台）还没有完成。原因是桌面控制授权被拒、Chrome 扩展未连接，自动化没法操作 Slack / Telegram 界面。用户决定上线后在生产上亲自连接验收。
- Slack 托管 OAuth：生产没有配置 managed Slack App 的 client 凭据，当前走"自带 App"。需要一键 OAuth 时，按 `deploy/messaging/slack-managed-app-manifest.yaml` 建 App，并配置 client id / secret。
- 飞书国际版（Lark）入口保持关闭，只开放飞书（Feishu）。
- 企业微信长连接由持有 WebSocket 租约的副本负责投递；生产是单副本，没问题，扩成多副本前要先看启动日志里的警告。

## 已修复的问题

- `b777ed30` 从混杂的工作树里带进了整份 `settings.json`：删掉了 `issue_statuses` / `properties` / `quick_actions` 等仍在使用的文案，还引用了没提交的 `emailChangeSupported`，导致 server 编译失败。`4024d4f2` 把文案还原为 `27c1b051` 的内容，只保留 7 条 `page.integrations_*` 和微信第一步文案，并删掉了那个字段。
- 钉钉的 Agent 连接按钮现在只在托管模式出现，Agent 集成页的旧测试没设置模式，CI 因此失败。`6fcc567a` 补上前提，并覆盖了自部署 / 禁用两种情况。

## 本地复现环境

- 环境：`cordy_messaging_live_20260910-301`（API 18381、daemon、Electron renderer 5475）；数据库 `orvilo_cordy_messaging_live_20260910_301`
- 模拟托管：`.env.worktree` 设置 `ORVILO_APP_URL=https://orvilo.aspectlylabs.com`（必须写在这里，因为 Makefile `include` 会覆盖环境变量）；`.env.local` 放六把本地随机 key（gitignored，600 权限）
- 本地测试 Agent：`IM Test Lead`（Claude runtime），已设为 `dev` 工作区 Lead
- 本地模拟托管的副作用：
  - 绑定链接和 `make dev-login` 的跳转都会指向生产域名。本地绑定时，要么把域名换成本地，要么直接调用 `POST /api/<platform>/binding/redeem`。
  - Web 必须有 Clerk 会话。Turbo 2 是严格 env 模式，不会把 `CLERK_SECRET_KEY` 传给 Next.js，所以 worktree 里要么绕开 turbo 直接跑 `apps/web` 的 `pnpm dev`，要么按项目约定用 Electron（`make up C=api,daemon,desktop`）。
- 本地的 Slack 安装已经断开，避免和生产抢同一个 Slack App 的事件（Socket Mode 每个事件只投递给一个连接）。
