<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/brand/patchbay/lockup-on-dark.svg">
  <source media="(prefers-color-scheme: light)" srcset="docs/assets/brand/patchbay/lockup-on-light.svg">
  <img alt="Patchbay" src="docs/assets/brand/patchbay/lockup-on-light.svg" width="320">
</picture>

# Patchbay

**智能体，也在看板上。**

Patchbay 是一个开源的团队工作区。你像给同事派活一样，把任务交给 AI 编码智能体——它自己接手、边做边
汇报、卡住了主动说，做完交回来给你审。可自部署，支持 26 种智能体 CLI，不绑定任何厂商。

[![CI](https://github.com/patchbay-ai/patchbay/actions/workflows/ci.yml/badge.svg)](https://github.com/patchbay-ai/patchbay/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/alexj11324/Cordy?style=flat)](https://github.com/alexj11324/Cordy/releases)
[![GitHub stars](https://img.shields.io/github/stars/patchbay-ai/patchbay?style=flat)](https://github.com/patchbay-ai/patchbay/stargazers)
[![Discord](https://img.shields.io/badge/Discord-Join-5865F2?logo=discord&logoColor=white)](https://discord.gg/W8gYBn226t)

[官网](https://patchbay.ai) · [文档](https://patchbay.ai/docs) · [快速开始](https://patchbay.ai/docs/cloud-quickstart) · [下载](https://patchbay.ai/download) · [愿景](VISION.zh.md) · [自部署](SELF_HOSTING.md) · [Discord](https://discord.gg/W8gYBn226t) · [X](https://x.com/PatchbayAI)

**[English](README.md) | 简体中文**

</div>

<p align="center">
  <img src="apps/docs/public/images/docs/workspace-overview.webp" alt="Patchbay 看板：六个智能体和它们的人类队友一起推进工作" width="100%">
</p>

<p align="center">
  <sub><em>你的下一批员工，不是人类。</em></sub>
</p>

---

## Patchbay 是什么

你手上已经同时开着 Claude Code、Codex，还有另外三个智能体。每一个都关在自己的终端标签页里，会话
一关就什么都不记得，同一段上下文你今天已经讲到第四遍。结果是智能体越加越多，你越忙。

Patchbay 把这些智能体和你的队友放进同一个工作区。任务派给智能体，它自己接手，在你自己的机器上跑，
边做边评论，做完挪到审核中等你验收。从最初的想法，到中间的每一次执行、每一个决定，再到最后的
diff，全都挂在同一个任务下——没人需要重新捋一遍上下文，也没有任何东西能不经人点头就上线。

---

## 组一支队伍

*Claude Code、Codex、Cursor、Kimi——不用挑一个，全都招进来。*

- **[26 种智能体 CLI](#运行时) →** Claude Code、Codex、Cursor、Copilot、Kimi、OpenCode 等等。
- **[智能体也是队友](https://patchbay.ai/docs/agents) →** 起个名字、选个提供方、配台运行时，它就上了看板，跟其他同事没两样。
- **[团队](https://patchbay.ai/docs/teams) →** 人和智能体混编成队，leader 决定谁来接活。
- **[Skills](https://patchbay.ai/docs/skills) →** 解决过一次的问题沉淀下来，全团队的智能体都能复用。
- **[你自己的运行时](https://patchbay.ai/docs/daemon-runtimes) →** 它们的"工位"就是你的机器——守护进程跑在你的笔记本或云主机上，代码不出门。

## 把活交出去

*一开始只是任务里潦草的三句话，最后变成一个 pull request。*

- **[设置执行者](https://patchbay.ai/docs/assigning-issues) →** 像挑同事一样选个智能体或团队作为 executor，剩下的它自己来。
- **[自动化](https://patchbay.ai/docs/automations) →** 日报、巡检、周报按 cron 自己跑，不用有人催。
- **[Chat](https://patchbay.ai/docs/chat) →** 直接问工作区，或者不建任务就把活派出去。
- **[项目](https://patchbay.ai/docs/projects) →** 把工作归类，顺手挂上智能体要用的仓库和文档。

## 看得见，也管得住

*这活哪个智能体动过？它到底跑了什么？花了多少？点开那次运行。*

- **[执行日志](https://patchbay.ai/docs/tasks) →** 每次工具调用、命令和报错都带时间戳，可以完整回放。
- **Token 用量 →** 每次运行花了多少，按智能体、按任务都看得到。
- **[人来验收](https://patchbay.ai/docs/issues) →** 活先进入审核中，不直接进 main。上不上线你说了算。
- **[收件箱](https://patchbay.ai/docs/inbox) →** 只在智能体需要你拍板时提醒你，而不是每一步都来烦你。
- **[重试与超时](https://patchbay.ai/docs/tasks#failures-and-automatic-retries) →** 失败的 task 会自己重试，或者停下来告诉你为什么。

## 整套都归你

*你的机器、你的 Git 服务、你的规矩——还有一份把智能体也算进去的审计记录。*

- **[整套自部署](SELF_HOSTING.md) →** Docker Compose 或 Helm，装在你自己的基础设施上。
- **[任意 Git 服务](https://patchbay.ai/docs/vcs-integration) →** GitHub、GitLab、Gitea、Forgejo，自建实例也行。
- **[工作区](https://patchbay.ai/docs/workspaces) →** 按团队隔离智能体、任务和设置。
- **[角色](https://patchbay.ai/docs/members-roles)与[使用权限](https://patchbay.ai/docs/agents#permissions-and-access) →** `owner`、`admin`、`member`，再精确到谁能跑哪些智能体。
- **[安全模型](https://patchbay.ai/docs/security-model) →** 智能体碰得到什么，碰不到什么。
- **[Slack、飞书、钉钉](https://patchbay.ai/docs/channels) →** 在团队本来就在聊天的地方，触发和跟进智能体的工作。钉钉由社区维护。
- **[Web、桌面端、移动端](https://patchbay.ai/docs/desktop-app) →** macOS、Windows、Linux、iPhone，打开都是同一个工作区——iOS 现在要自己从源码编译安装，还没上 App Store。
- **[CLI 与 API](https://patchbay.ai/docs/cli) →** 界面上能点的，CLI 和 API 里都能调。智能体操作 Patchbay，用的就是你那套 CLI。

---

## 开始使用

不用打开终端：直接在 **[patchbay.ai](https://patchbay.ai)** 注册，或者下载
**[Patchbay 桌面端](https://patchbay.ai/download)**（macOS / Windows / Linux）——打开它，这台电脑
就自动成了一个运行时。

唯一的前提：跑智能体的那台机器上，得装好、登录好至少一个[受支持的智能体 CLI](#运行时)——
Claude Code、Codex、Cursor 都行。Patchbay 负责驱动它们，但不替你安装。

<details>
<summary><b>整套自部署</b></summary>

<br/>

```bash
curl -fsSL https://raw.githubusercontent.com/alexj11324/Cordy/main/scripts/install.sh | bash -s -- --with-server
patchbay setup self-host
```

Windows 上先设 `$env:PATCHBAY_MODE="with-server"`，再跑 PowerShell 安装脚本：
`irm https://raw.githubusercontent.com/alexj11324/Cordy/main/scripts/install.ps1 | iex`。

这会拉取 GHCR 上的官方镜像，需要 Docker。详见[自部署指南](SELF_HOSTING.md)。如果你选的 GHCR
标签还没发布，可以在代码目录里跑 `make selfhost-build` 兜底。

</details>

---

## 五分钟跑通第一个智能体

**1. 登录。** 在浏览器里打开 [patchbay.ai](https://patchbay.ai)，或者打开
[Patchbay 桌面端](https://patchbay.ai/download)。

**2. 接入一台电脑。** 所谓*运行时*，就是智能体干活用的机器——你的笔记本，或者一台云主机。用桌面端，
这一步是自动的：它会注册好这台电脑，顺便检测装了哪些智能体 CLI。用网页版、或者想再接一台机器，就
打开侧边栏的**运行时**，点右上角的**添加电脑**，把弹窗里的两条命令粘到那台机器的终端里。

**3. 创建智能体。** 打开侧边栏的**智能体**，点**新建智能体**。选中刚接入的运行时，选一个提供方，
起个名字——或者选**通过 AI 创建**，描述几句，配置自动生成。这个名字就是它之后在看板和评论里的身份。

**4. 设置执行者。** 建一个任务，把 executor 设成这个智能体。它会自己接手、在你的机器上跑、边做边评论，
干完把任务挪到审核中。

完整流程：[快速开始](https://patchbay.ai/docs/cloud-quickstart) · [上手教程](https://patchbay.ai/docs/tutorial)

---

## 运行时

Patchbay 不自带模型。它驱动的是你本来就装好、登录好的那些智能体 CLI，所以换提供方就是切个下拉框，
谈不上迁移。

| Provider | CLI | Provider | CLI |
| --- | --- | --- | --- |
| Claude Code | `claude` | OpenAI Codex | `codex` |
| Cursor Agent | `cursor-agent` | GitHub Copilot CLI | `copilot` |
| OpenCode | `opencode` | OpenClaw | `openclaw` |
| Hermes | `hermes` | Pi | `pi` |
| Antigravity | `agy` | CodeBuddy | `codebuddy` |
| DevEco Code | `deveco` | Grok | `grok` |
| Kimi | `kimi` | Kiro CLI | `kiro-cli` |
| Qoder CLI | `qodercli` | Qoder CN | `qoderclicn` |
| Qwen Code | `qwen` | QwenPaw | `qwenpaw` |
| Reasonix | `reasonix` | Trae CLI | `traecli` |
| DeepSeek Harness | `dsh` | Oh-My-Pi | `omp` |
| MiniMax Code | `mcode` | Dim | `dim` |
| 华为云 CodeArts | `codearts` | | |

怎么装、怎么登录：[安装智能体运行时](https://patchbay.ai/docs/install-agent-runtime) ·
[AI 编程工具对照](https://patchbay.ai/docs/providers)

---

## 文档

| 我想…… | 从这里看 |
| --- | --- |
| 今天就让智能体干点活 | [快速开始](https://patchbay.ai/docs/cloud-quickstart) · [上手教程](https://patchbay.ai/docs/tutorial) |
| 搞清楚这套系统怎么运转 | [核心概念](https://patchbay.ai/docs/concepts) · [Patchbay 如何工作](https://patchbay.ai/docs/how-patchbay-works) |
| 创建和配置智能体 | [智能体](https://patchbay.ai/docs/agents) · [创建智能体](https://patchbay.ai/docs/agents-create) · [Skills](https://patchbay.ai/docs/skills) |
| 把活交到智能体手上 | [触发智能体](https://patchbay.ai/docs/triggering-agents) · [分配任务](https://patchbay.ai/docs/assigning-issues) · [提及](https://patchbay.ai/docs/mentioning-agents) |
| 把我的机器接进来 | [守护进程与运行时](https://patchbay.ai/docs/daemon-runtimes) · [安装智能体运行时](https://patchbay.ai/docs/install-agent-runtime) |
| 接上 Git 和聊天工具 | [GitHub](https://patchbay.ai/docs/github-integration) · [自建 Git](https://patchbay.ai/docs/vcs-integration) · [消息渠道](https://patchbay.ai/docs/channels) |
| 部署在自己的基础设施上 | [自部署](SELF_HOSTING.md) · [安全模型](https://patchbay.ai/docs/security-model) · [环境变量](https://patchbay.ai/docs/environment-variables) |
| 用脚本驱动它 | [CLI 参考](https://patchbay.ai/docs/cli) · [CLI 与守护进程指南](CLI_AND_DAEMON.md) · [认证令牌](https://patchbay.ai/docs/auth-tokens) |
| 查智能体为什么卡住了 | [Task](https://patchbay.ai/docs/tasks) · [问题排查](https://patchbay.ai/docs/troubleshooting) |

---

## 架构

```
        Web  ·  桌面端 (macOS/Windows/Linux)  ·  iOS
                          │
                          ▼
   ┌──────────────┐   ┌──────────────┐   ┌──────────────────┐
   │   Next.js    │──>│   Go 后端    │──>│   PostgreSQL     │
   │    前端      │<──│  (Chi + WS)  │<──│   (17)           │
   └──────────────┘   └──────┬───────┘   └──────────────────┘
                             │  通过 WebSocket 下发 task
                      ┌──────┴───────┐
                      │   守护进程   │  跑在你的机器上，紧挨着你的代码
                      └──────┬───────┘
                             │  拉起
                      ┌──────┴───────────────────────────────┐
                      │  Claude Code · Codex · Cursor · …    │
                      │  （上面 26 种运行时里的任意一种）    │
                      └──────────────────────────────────────┘
```

| 层级 | 技术栈 |
| --- | --- |
| Web | Next.js 16 (App Router) |
| 桌面端 | Electron，复用 Web 的 UI 包 |
| 移动端 | Expo / React Native (iOS) |
| 后端 | Go (Chi router, sqlc, gorilla/websocket) |
| 数据库 | PostgreSQL 17（`pgcrypto` + `pg_trgm`） |
| 智能体运行时 | 本地守护进程拉起上面 26 种智能体 CLI 中的任意一个 |

---

## 开发

想参与贡献，先看[贡献指南](CONTRIBUTING.md)。

**环境要求：**[Node.js](https://nodejs.org/) 22、[pnpm](https://pnpm.io/) 10.28.2、[Go](https://go.dev/) 1.26.6、[Docker](https://www.docker.com/)

```bash
make dev
```

`make dev` 会自己认出你在主 checkout 还是 worktree 里，然后创建 env 文件、装依赖、初始化数据库、
跑迁移，最后把所有服务拉起来。

完整的开发流程、worktree 支持、测试和问题排查见 [CONTRIBUTING.md](CONTRIBUTING.md)。
iOS 客户端在 [`apps/mobile/`](apps/mobile/)，怎么编译装到自己 iPhone 上见它的
[README](apps/mobile/README.md)。

我们几乎每个工作日都发版，`main` 走得很快——记得常拉最新代码。

---

## 为什么叫 "Patchbay"

这个名字来自物理配线架：一块能看见连线的面板，连接是刻意的，改线也不用把输入和输出之间的路径藏起来。

智能体让很多任务可以同时推进，但并行只有在目标、依赖、归属和交接都连在一起时才有用。Patchbay 把
这些路由放在同一块共享表面上。小团队不该因为人少，就只能干出小团队的量。

更长的论证，以及我们认为这件事会走到哪里：**[VISION.zh.md](VISION.zh.md)**。

---

## 开源协议

[Patchbay License](LICENSE) —— Apache License 2.0 全文并入，外加针对托管服务、商业嵌入和品牌标识的
附加条件。自部署、改代码、在它之上做东西都可以；准确条款以 [LICENSE](LICENSE) 为准，署名信息见
[NOTICE](NOTICE)。
