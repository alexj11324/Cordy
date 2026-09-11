# Lessons

## 验证共享 UI 必须走 Electron，不要在本地起 Next.js dev server

**日期**: 2026-09-11
**场景**: 改造 `packages/views/chat/` 的聊天面板

**纠正**: 用户明确要求「走 electron 编译验证，不允许在本地使用 nextjs 开发」。

**为什么**: 项目 `CLAUDE.md` 的 Verification 节已经写明 —— 「Shared UI changes must be visually accepted: **prefer Electron** (`make up C=desktop`)」。`packages/views` 是 web 与 desktop 共享的代码，Electron 才是首选验收客户端。我先起了 `next dev` 属于选错了验收通道。

**怎么做**:
- 验证 `packages/views` 的改动 → `pnpm --filter @orvilo/desktop typecheck` / `build`，或 `make up C=desktop`
- Next.js dev server 仅在 Electron 起不来、或 renderer 损坏时作为回退
- 报验收结论时必须说明**在哪个客户端看的**
- 注意 `make up` 需要 Docker（PostgreSQL）；Docker 未启动时用 Electron 自己的编译/类型检查路径，不要顺手退回 Next.js

---

## 这台机器的 `npm` 是 bun 的 alias，会重写 pnpm 配置

**日期**: 2026-09-11

**现象**: 运行 `npm install` 后，根 `package.json` 被改写 —— `pnpm.overrides` 被提到顶层，并凭空注入 `workspaces.catalog`。原因是 `npm` 被 alias 成 `bun`，而 `bun install` 会主动把 `pnpm-workspace.yaml` 的 catalog 迁移成 npm/bun 原生格式。

**怎么做**:
- 本仓库一律用 `pnpm add` / `pnpm install`，禁止在仓库内跑 `npm` / `bun install`
- 装完立即 `git diff package.json` 校验没被改写
- 依赖解析实验放到隔离目录外做，或用 CDN 直接读包，避免误触仓库

---

## pnpm 10 不再读取 package.json 的 `pnpm` 字段

**日期**: 2026-09-11

**现象**: `pnpm 10.28.2` 警告 `The "pnpm" field in package.json is no longer read`，导致 `pnpm.overrides` 与 `pnpm.onlyBuiltDependencies` **静默失效**。后者会让依赖的 `postinstall` 被拦截 —— `@lobehub/editor` 正是靠 postinstall 给 `lexical@0.42.0` 打补丁。

**怎么做**:
- 新增 build script 白名单要写进 `pnpm-workspace.yaml` 的 `onlyBuiltDependencies`，不是 package.json
- 仓库现有的 `pnpm.overrides` 迁移属于独立问题，动它之前先确认影响面
