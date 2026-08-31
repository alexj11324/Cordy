<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/brand/patchbay/lockup-on-dark.svg">
  <source media="(prefers-color-scheme: light)" srcset="docs/assets/brand/patchbay/lockup-on-light.svg">
  <img alt="Patchbay" src="docs/assets/brand/patchbay/lockup-on-light.svg" width="320">
</picture>

**让编码智能体的工作从需求顺畅流转到审核，全程不丢上下文。**

[![CI](https://github.com/patchbay-ai/patchbay/actions/workflows/ci.yml/badge.svg)](https://github.com/patchbay-ai/patchbay/actions/workflows/ci.yml)

**[English](README.md) | 简体中文**

</div>

Patchbay 是面向编码智能体工作的开源控制平面。需求、执行、决策、结果和审核状态都保留在同一条工作记录中，智能体则运行在你控制的基础设施上。

名字来自实体 patch bay：它把输入和输出清楚地接在一起，也让中间经过的路径保持可见。

## Patchbay 能做什么

- **让上下文保持完整。** 需求、智能体执行、进度、阻塞和审核都归在同一个任务下。
- **在代码所在的位置运行智能体。** 本地守护进程会在你的电脑或自有运行时上启动已登录的编码智能体 CLI。
- **让执行过程可检查。** 直接查看事件、日志、重试、超时和用量，不必事后拼凑终端会话。
- **保留人工控制。** 完成的工作先交回审核，再决定是否接受或发布。
- **覆盖不同工作端。** 仓库包含 Web、桌面端、iOS 客户端，以及 CLI 和 API。
- **支持自部署。** 应用与 PostgreSQL 都可以运行在自己的基础设施上。

Patchbay 不内置模型或编码智能体。它负责协调由你单独安装并完成登录的兼容智能体 CLI。

## 架构

```text
 Web · 桌面端 · iOS
          │
          ▼
 Next.js / 共享 UI ───────► Rust API + WebSocket 服务
                                      │
                                      ▼
                              PostgreSQL + pgvector
                                      ▲
                                      │ task 事件
                                本地智能体守护进程
                                      │
                                      ▼
                              已安装的编码智能体 CLI
```

| 层级       | 当前实现                                        |
| ---------- | ----------------------------------------------- |
| Web        | Next.js App Router                              |
| 桌面端     | Electron，复用 Web UI 包                        |
| 移动端     | Expo / React Native iOS 客户端                  |
| 后端       | Rust、Axum、SQLx 和 WebSocket                   |
| 数据库     | PostgreSQL 17 + pgvector                        |
| 本地运行时 | Rust CLI 和守护进程，负责启动已安装的智能体 CLI |

Rust server、CLI、迁移 runner 和 backfill 二进制是生产入口。

## 从源码运行

### 环境要求

- Node.js 22+
- pnpm 10.28.2
- stable Rust toolchain
- sccache（macOS 可运行 `brew install sccache`）
- Docker 与 Docker Compose，或本机 PostgreSQL 15+

```bash
git clone https://github.com/patchbay-ai/patchbay.git patchbay
cd patchbay
pnpm dev
```

`pnpm dev`（POSIX 环境也可使用便捷别名 `make dev`）是 macOS、Linux 和 Windows
唯一的完整 Desktop 开发入口。它会创建隔离的
worktree 环境，通过共享 pnpm store 安装依赖，启动 PostgreSQL、执行迁移，等待
本地 Rust 后端和数据库就绪，准备与当前源码匹配的 dev runtime（CLI、后端、
迁移 runner），验证本机智能体检测
以及 Telegram/微信加密配置，全部通过后才打开带热更新的 Electron。

dev runtime 会按 Rust 源码、Cargo manifests/Cargo.lock、toolchain、target、架构、
profile 和构建变量存入用户级缓存。重复启动以及 Rust 源码相同的新 worktree 会
直接复用校验过的 CLI、后端和迁移 runner，不再编译 Rust；未命中时才一次性执行
增量 dev 构建。安装 `sccache`
可以跨 worktree 共享编译对象，但每个 worktree 的 `server-rs/target` 仍保持隔离。
可用 `pnpm dev:doctor` 重新运行能力诊断；独立的 Next.js Web 客户端使用
`pnpm dev:web:next`。
`PATCHBAY_POSTGRES_RUNTIME=auto` 只会在 Compose 固定发布的
`localhost:5432` 上选择 Docker；如果该地址存在歧义，请显式设为 `native` 或
`docker`。

需要显式构建时运行：

```bash
make build
pnpm build
```

`pnpm build` 只构建前端和 Electron bundle，不编译 Rust。只有验证安装包、
签名/公证、自动更新、内置 CLI 或正式发布时，才运行
`pnpm --filter @patchbay/desktop package`。该全量路径会构建 release Rust CLI
（或使用 CI 中经过 checksum 校验、精确对应 commit 的 artifact）
和安装包，可能耗时几十分钟，不应该用于日常修改后的刷新验证。完整选择表见
[贡献指南](CONTRIBUTING.md#desktop-app-local-testing)。

## 文档

- [自部署](SELF_HOSTING.md)
- [CLI 与智能体守护进程](CLI_AND_DAEMON.md)
- [贡献指南](CONTRIBUTING.md)
- [高级自部署配置](SELF_HOSTING_ADVANCED.md)
- [迁移审计台账](tasks/go-to-rust-migration-audit.md)

CLI、package scope、Rust crate、部署产物、环境变量、存储键和应用标识现已统一使用 Patchbay。升级时会在兼容边界迁移现有本地配置与浏览器登录会话。

## 开源协议

Patchbay 按 [LICENSE](LICENSE) 中的条款分发，署名信息见 [NOTICE](NOTICE)。
