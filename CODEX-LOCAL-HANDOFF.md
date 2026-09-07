# Codex 本地交接（PR #779）

**给 Codex / 本地操作者。云端 Agent 没有 Docker、Electron、Cloudflare、源站权限。读完后按下面做；全部完成后把本文件从本 PR 删掉，不要合进 `main`。**

- PR：https://github.com/alexj11324/Cordy/pull/779
- 分支：`cursor/orvilo-rebrand-packages-031f`
- 基线：`main`
- 当前 HEAD（写本文时）：`d815c6878` `test(auth): reject the legacy Patchbay JWT default`
- 仓库仍叫 `alexj11324/Cordy`。生产 Compose 项目名仍是 `cordy` / `cordy632-*`。不要改这两处。

## 产品决策（不要推翻）

用户要求**干净移交**：所有产品身份从 Patchbay 改成 Orvilo，不要兼容层、不要双写、不要从旧 host 做 301。

| 项 | 现值 |
| --- | --- |
| 产品站 / 文档 | `https://orvilo.aspectlylabs.com`（文档同 host `/docs`） |
| API | `https://api.aspectlylabs.com`（**故意没改主机名**） |
| Accounts | `https://accounts.aspectlylabs.com`（**故意没改主机名**） |
| 发件 | `noreply@orvilo.aspectlylabs.com`（用户否决了保留旧邮箱） |
| CLI / Homebrew / goreleaser | `orvilo` |
| Helm / OCI | `deploy/helm/orvilo`，`oci://ghcr.io/alexj11324/charts/orvilo` |
| GHCR | `ghcr.io/alexj11324/orvilo-{backend,web,docs,auth-broker}` |
| 协议 / 插件 / cookie / header / 家目录 | `orvilo://`、`orvilo.plugin.json`、`orvilo_auth`、`X-Orvilo-*`、`~/.orvilo` |
| Token / Redis | `ovg_` `ovy_` `ovd_` `ovl_`；`ovy:` |
| 官方云判断 | 前端 host `orvilo.aspectlylabs.com`；daemon 官方 API host `api.aspectlylabs.com`（硬编码，**不能只靠 Cloudflare 覆盖**） |

域名：**DNS / 证书 / 运行时 env 可以在云端改；nginx `server_name`、CLI/桌面默认云地址、官方云判断、生产探针 Host 已经写进本 PR。只改 Cloudflare 不够。**

`COOKIE_DOMAIN` 不要设成只有产品站。Web 是 `orvilo.aspectlylabs.com`，API 是 `api.aspectlylabs.com`，通常用 `.aspectlylabs.com`。

## 不要动

- 历史 `server/migrations/*.sql`（除已新增的 `594_orvilo_identity`）保持字节不变
- GitHub 仓库名 `Cordy`
- 生产 Compose 项目 `cordy` / `cordy632-*`
- reserved slug `patchbay`（阻止别人抢旧名；同时已加 `orvilo`）
- JWT denylist 里的 `patchbay-dev-secret-change-in-production`（安全拒绝旧默认值）
- 安装脚本里的 `/var/lib/patchbay-production` 等旧路径（一次性 `mv` 需要）
- 测试里的退役 URL 针、历史 issue 引用 `patchbay-ai/patchbay#NNNN`、GitHub 夹具 org 名

## 云端已经做过 / 测过

代码切流已进分支。云端跑过且通过：

```bash
bash scripts/selfhost-links.test.sh
python3 -m unittest deploy/origin/production_deploy_test.py
node --test scripts/auth-broker-release-contract.test.mjs \
  scripts/production-deployment-contract.test.mjs \
  scripts/verify-production-deployment.test.mjs \
  scripts/verify-production-browser.test.mjs
# Go 1.26.6
go test ./internal/auth ./internal/service -count=1
go build ./cmd/orvilo ./cmd/server
pnpm exec vitest run \
  apps/desktop/src/shared/callback-protocol.test.ts \
  apps/auth-broker/lib/desktop-handoff.test.ts \
  apps/auth-broker/lib/contract.test.ts \
  packages/core/api/client.test.ts
```

云端**没跑**（缺 Docker / Electron / 源站）：

- `bash scripts/selfhost-config.test.sh`
- `pnpm typecheck` / 全量 `pnpm test` / `make test`
- `make up C=desktop`（产品行为必须在 Electron 里验）
- Cloudflare / 源站 nginx / `install-production-deploy.sh` / Clerk / Resend
- 本机已有名为 `patchbay` 的数据库上跑 `make seed-dev`（Makefile 默认库名已是 `orvilo`）

GitGuardian 扫到 `.github/workflows/ci.yml` 和 `.env.example` 的 `POSTGRES_PASSWORD: orvilo`，是本地占位符，不是泄露的生产密钥。

---

## 你要做的（按顺序）

### 1. 检出

```bash
git fetch origin cursor/orvilo-rebrand-packages-031f
git checkout cursor/orvilo-rebrand-packages-031f
git pull --ff-only origin cursor/orvilo-rebrand-packages-031f
```

需要时再 `git fetch origin main`。不要无故 rebase；有冲突再处理。

### 2. 本地验证（必须）

产品改动以 **Desktop / Electron** 为准，不要只开浏览器。

```bash
make up C=desktop
```

签入后在桌面客户端走一遍：登录、工作区、issue、设置、CLI `orvilo`、deep link `orvilo://`。确认窗口/协议不再叫 Patchbay。

然后：

```bash
bash scripts/selfhost-config.test.sh
pnpm typecheck
cd server && go test ./internal/service -run TestBuiltinSkills -count=1
cd server && go test ./internal/handler -run 'Guest|Handoff' -count=1
make test   # 若时间不够，至少跑与身份/邮件/guest/handoff/deploy 相关的包
pnpm test   # 至少覆盖 callback-protocol、Linear source_orvilo、auth-broker
```

失败就修、commit、push 到同一分支。修的时候仍然禁止兼容层和双路径。

本地库若仍叫 `patchbay`，`make seed-dev` 会失败。要么迁到 `orvilo`，要么按 `594_orvilo_identity` 迁现有库，不要改历史 migration 文件。

### 3. 源站切流（有 origin / Cloudflare 权限时，合进 main **之前**）

旧网关 allowlist 还是 `patchbay-*`，**第一次**打 `orvilo-*` 镜像的 Action 部署会直接被拒。顺序：

1. Cloudflare：`orvilo.aspectlylabs.com` DNS + TLS。**不要**给 `patchbay.aspectlylabs.com` 做 301。
2. 源站安装 `deploy/origin/nginx/aspectlylabs-origin.conf`（`server_name orvilo.aspectlylabs.com`）。
3. **先于**第一次带新镜像的 Action 部署，执行：
   ```bash
   sudo deploy/origin/install-production-deploy.sh <deploy-key.pub>
   ```
   会把 `/var/lib/patchbay-production` 等迁到 `/var/lib/orvilo-production`，二进制改为 `orvilo-production-deploy`，并 down 掉遗留的 `patchbay-auth-broker`。
4. 源站 env 快照并改成：
   - `ORVILO_APP_URL=https://orvilo.aspectlylabs.com`
   - `FRONTEND_ORIGIN=https://orvilo.aspectlylabs.com`
   - `CORS_ALLOWED_ORIGINS` 含新产品 origin
   - `CLERK_AUTHORIZED_PARTIES=https://accounts.aspectlylabs.com,https://orvilo.aspectlylabs.com`
   - `RESEND_FROM_EMAIL=noreply@orvilo.aspectlylabs.com`
   - `COOKIE_DOMAIN=.aspectlylabs.com`（不要只写产品 host）
   - `ORVILO_PUBLIC_URL=https://api.aspectlylabs.com`（API 主机名不变）
5. Clerk dashboard：authorized parties 加上 `https://orvilo.aspectlylabs.com`。
6. Resend：验证 `orvilo.aspectlylabs.com` 发信域。
7. Nebula staging：compose/文档已改成 `orvilo-nebula-staging`、`orvilo-staging*.nebula-spaces.com`。源站目录、证书、Secret Manager 要一起迁，不能只改仓库。
8. 内部 staging Desktop/Mobile：`apps/desktop/.env.staging` 和 `apps/mobile/.env.staging` 已指向 `orvilo-api.copilothub.ai` / `orvilo-app.copilothub.ai`。确认这两条 DNS 存在，否则 `pnpm dev:desktop:staging` 是死的。

合 `main` 之后，`.github/workflows/aspectlylabs-production-images.yml` 才会在 CI 绿了以后出新镜像（没有 `workflow_dispatch`）。网关没换 allowlist 之前不要合。

### 4. 收尾

- 从本 PR **删除** `CODEX-LOCAL-HANDOFF.md` 再请求 review/merge。
- 不要把本文件留在 `main`。
- 用户没让你 merge 就不要 merge。

## 关键路径（修 bug 时）

- 官方云：`server/internal/handler/config.go`（`orvilo.aspectlylabs.com`）、`server/internal/daemon/config.go`（`api.aspectlylabs.com`）
- CLI 默认：`server/cmd/orvilo/cmd_agent.go` `defaultCloudServerURL` / `defaultCloudAppURL`
- 邮件：`server/internal/service/email.go`；测试期望 From 为 `noreply@orvilo.aspectlylabs.com`
- 生产探针 Host：`deploy/origin/production_deploy.py`
- nginx：`deploy/origin/nginx/aspectlylabs-origin.conf`
- 迁移：`server/migrations/594_orvilo_identity.up.sql`（列 `patchbay_*` → `orvilo_*`，origin `orvilo`，GUC `orvilo.*`）
- Guest hash（`ovg_abc`）：`d2e1286c4ff3d6620bb46238360ce3a54c86527ce01512da105291933b513fdd`

## 中文产品文案

改 UI/文档中文时先读 `apps/docs/content/docs/developers/conventions.zh.mdx`。
