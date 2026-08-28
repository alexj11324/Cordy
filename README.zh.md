<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/brand/patchbay/lockup-on-dark.svg">
  <source media="(prefers-color-scheme: light)" srcset="docs/assets/brand/patchbay/lockup-on-light.svg">
  <img alt="Patchbay" src="docs/assets/brand/patchbay/lockup-on-light.svg" width="320">
</picture>

**让编码智能体的工作从需求顺畅流转到审核，全程不丢上下文。**

[![CI](https://github.com/alexj11324/Cordy/actions/workflows/ci.yml/badge.svg)](https://github.com/alexj11324/Cordy/actions/workflows/ci.yml)

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

| 层级 | 当前实现 |
| --- | --- |
| Web | Next.js App Router |
| 桌面端 | Electron，复用 Web UI 包 |
| 移动端 | Expo / React Native iOS 客户端 |
| 后端 | Rust、Axum、SQLx 和 WebSocket |
| 数据库 | PostgreSQL 17 + pgvector |
| 本地运行时 | Rust CLI 和守护进程，负责启动已安装的智能体 CLI |

Rust server、CLI、迁移 runner 和 backfill 二进制是生产入口。

## 从源码运行

### 环境要求

- Node.js 22+
- pnpm 10.28.2
- stable Rust toolchain
- Docker 与 Docker Compose，用于 PostgreSQL

```bash
git clone https://github.com/alexj11324/Cordy.git patchbay
cd patchbay
make dev
```

`make dev` 会按需创建本地环境、安装依赖、启动 PostgreSQL、执行迁移，并启动 Rust 后端和 Web 客户端。

需要显式构建时运行：

```bash
make build
pnpm build
```

## 文档

- [自部署](SELF_HOSTING.md)
- [CLI 与智能体守护进程](CLI_AND_DAEMON.md)
- [贡献指南](CONTRIBUTING.md)
- [高级自部署配置](SELF_HOSTING_ADVANCED.md)
- [迁移审计台账](tasks/go-to-rust-migration-audit.md)

部分内部 package、可执行文件、环境变量和存储名称仍使用之前的产品标识。涉及公开契约或持久化数据时暂不强行修改，后续会按独立的重命名边界处理。

## 开源协议

Patchbay 按 [LICENSE](LICENSE) 中的条款分发，署名信息见 [NOTICE](NOTICE)。
