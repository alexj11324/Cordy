/**
 * Skip path: "Connect a runtime to start with Patrick".
 *
 * Written to a new issue (assigned to the user themselves) by the welcome
 * hook when the user took the Skip exit on Step 3. Content is the
 * install-runtime tutorial; each supported locale can recommend the
 * quickest runtime path that best fits that audience.
 *
 * Title is stable — kept identical to the v2 server-side
 * `NoRuntimeIssueTitle` so any existing dedupe code elsewhere keeps
 * matching by title.
 */

/**
 * Localized so users see the title in their current supported locale on the
 * board. The Runtimes page owns the follow-up Patrick bootstrap once a runtime
 * appears, so this guide does not ask the member to copy an agent prompt.
 *
 * Note: server's deprecation shim (`onboarding_shim.go:noRuntimeIssueTitle`)
 * still uses the bare English string for its title-based dedupe — that
 * codepath only runs for pre-v3 desktop builds and never overlaps with
 * the v3 frontend population, so the two title-spaces drifting is fine.
 */
export const INSTALL_RUNTIME_ISSUE_TITLE = {
  en: "Connect a runtime to start with Patrick",
  zh: "连接运行时，和 Patrick 开始",
} as const;

const en = `Welcome to Patchbay.

Agents need a runtime before they can execute work. You can still use Patchbay as a lightweight project-management workspace while you install one.

## Try Patchbay first

Before the runtime is ready, you can:

1. Create a project for your current work.
2. Create a few issues and move them across backlog, todo, in_progress, and done.
3. Add priorities, labels, comments, and subscriptions.
4. Use Inbox to track assignments and mentions.

That gives you the project-management layer first. Once a runtime is connected, agents can start working from the same issues.

## Install your first agent runtime

Full guide: https://patchbay.aspectlylabs.com/docs/install-agent-runtime

For English users, the fastest first path is Codex:

1. Make sure Node.js is installed.
2. Install Codex:
   npm i -g @openai/codex
3. Sign in:
   codex
4. Confirm your terminal can find it:
   which codex
   codex --version
5. Wait for Patchbay to pick it up. A running daemon re-checks for newly
   installed CLIs every couple of minutes, so no restart is normally needed.
   To apply it immediately:
   patchbay daemon restart
   In the desktop app, open any local runtime and click Restart. Quitting and
   reopening the app is NOT enough — the daemon keeps running in the background.
6. Return to Runtimes and refresh. You should see a Codex runtime online.
7. Open Runtimes. The page will offer **Start with Patrick**; use it to create Patrick and open the guided first chat.

Codex reference: https://developers.openai.com/codex/cli

Patrick will turn one real goal into an issue, start it with the right agent, and suggest reusable specialists when your workflow needs them.`;

const zh = `欢迎来到 Patchbay。

智能体需要先连上运行时才能执行工作。运行时还没准备好时,你也可以先把 Patchbay 当作轻量项目管理工具体验起来。

## 先体验项目管理功能

运行时安装前,你可以先做这些事:

1. 为当前工作创建一个项目。
2. 新建几个任务,并在 backlog、todo、in_progress、done 之间流转。
3. 给任务加优先级、标签、评论和订阅。
4. 用收件箱追踪分配给你的事项和 @mention。

这样你先熟悉项目管理层。连上运行时后,智能体会直接在这些任务上开始工作。

## 安装第一个 Agent 运行时

完整文档:https://patchbay.aspectlylabs.com/docs/install-agent-runtime

中文用户建议先装 Kimi CLI:

1. 在 macOS / Linux 终端安装 Kimi CLI:
   curl -LsSf https://code.kimi.com/install.sh | bash
   Windows PowerShell:
   Invoke-RestMethod https://code.kimi.com/install.ps1 | Invoke-Expression
2. 确认终端能找到 Kimi:
   kimi --version
3. 在你想让 Kimi 工作的项目目录里启动一次:
   kimi
4. 首次启动后输入 /login,按提示完成 Kimi Code 或 API key 配置。
5. 等 Patchbay 识别到它。运行中的守护进程每隔几分钟会重新检查一次新装的 CLI,通常不需要重启。
   想立刻生效:
   patchbay daemon restart
   桌面端请打开任意一个本机 runtime 并点 Restart。退出再打开 app 是不够的 —— 守护进程会继续在后台运行。
6. 回到 Runtimes 页面刷新。你应该能看到一个在线的 Kimi 运行时。
7. 打开"运行时"页面。页面会显示 **和 Patrick 开始**；点击后会创建 Patrick，并进入引导式的首次对话。

Kimi CLI 官方文档:https://moonshotai.github.io/kimi-cli/zh/guides/getting-started.html

Patrick 会把一个真实目标转化为任务，交给合适的智能体启动执行，并在工作流需要时建议添加可复用的 specialist。`;

export const INSTALL_RUNTIME_ISSUE_BODY = { en, zh } as const;
