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

**补充（2026-09-11，同日实测）**: 上面那条修法**已应用**，但**不足以保证 postinstall 真的执行** ——
`pnpm-workspace.yaml:61` 的 `onlyBuiltDependencies: ['@lobehub/editor']` 位置正确，而一次
`pnpm install` 仍然把它列进了 `Ignored build scripts` 横幅，且 patch 确实没打上。
**没有确认机制，所以不写机制**：当时安装跑在 `CI=true` 下（为绕过
`ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY` 不得不加），这是一个**候选**而非结论；
另一个可疑点在于 `node_modules` 此前由**另一个 pnpm store**（`~/.cache/codex-pnpm-store/v10`）
构建，而新 pnpm 默认 `~/Library/pnpm/store/v11`，`node_modules/.modules.yaml` 可能是旧状态。
值得注意的是矛盾之处：`.modules.yaml` 的 `ignoredBuilds` **只**记了 `msw`/`sharp`/
`electron-winstaller`/`unrs-resolver`，**没有** `@lobehub/editor`，可是它的脚本确实没跑。

**怎么做（补充）**:
- **装完必须校验 patch，不能只看警告横幅**。按 `@lobehub/editor` 自带
  `scripts/postinstall-lexical-patch.cjs` 里**硬编码的 sha256** 校验**全部 8 个**文件 ——
  `lexical` 的 `Lexical.{dev,prod}.{js,mjs}` 四个，加上 `@lexical/yjs` 的
  `LexicalYjs.{dev,prod}.{js,mjs}` 四个（脚本里两组 `PATCH_CONFIGS` 分别标着
  `packageName: 'lexical'` / `packageName: '@lexical/yjs'`）；不匹配就手动跑该脚本再校验。
  只查前四个会「看起来干净」却漏掉协同用的那一份。
  失败是**静默**的：只有一行 warning，而 `lexical` 未打补丁会让编辑器行为异常且难以归因。
- 该脚本用「临时文件 + rename」写，因此**不会**污染 pnpm store；store 里那份是未打补丁的，
  这既解释了为什么重装会退回未打补丁状态，也说明重跑脚本是安全且幂等的。
