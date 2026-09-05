import "@fontsource-variable/inter";
import "@fontsource-variable/inter/wght-italic.css";
import "@fontsource-variable/source-serif-4";
import "@fontsource-variable/source-serif-4/wght-italic.css";
import "@fontsource-variable/geist-mono";
import "./style.css";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  QueryClient,
  QueryClientProvider,
  onlineManager,
} from "@tanstack/react-query";
import { createStore } from "zustand/vanilla";
import { ApiClient, setApiInstance } from "@patchbay/core/api";
import {
  NavigationProvider,
  type NavigationAdapter,
} from "@patchbay/views/navigation";
import { I18nProvider } from "@patchbay/core/i18n/react";
import { WorkspaceSlugProvider } from "@patchbay/core/paths";
import { workspaceKeys } from "@patchbay/core/workspace/queries";
import { agentTaskSnapshotKeys } from "@patchbay/core/agents/queries";
import { propertyKeys } from "@patchbay/core/properties";
import { issueStatusKeys } from "@patchbay/core/issue-statuses/queries";
import { ViewStoreProvider } from "@patchbay/core/issues/stores/view-store-context";
import { viewStoreSlice } from "@patchbay/core/issues/stores/view-store";
import { BoardCardContent } from "@patchbay/views/issues/board-card";
import { RESOURCES } from "@patchbay/views/locales";
import {
  agent,
  issue,
  workspace,
  taskSnapshot,
  type ExecutionState,
} from "./fixtures";

// No CoreProvider, auth bootstrap, daemon, or WebSocket provider.
// Even explicitly enabled production queries remain paused. CSP additionally
// blocks fetch/XHR/WebSocket/beacon regardless of query configuration.
onlineManager.setOnline(false);
// Avatar URL resolution needs getBaseUrl even for null avatars. All other API
// access fails locally, before any request can be constructed.
setApiInstance(
  new Proxy(new ApiClient(""), {
    get(_target, key) {
      if (key === "getBaseUrl") return () => "";
      throw new Error(`Card preview blocked API access: ${String(key)}`);
    },
  }),
);
const navigation: NavigationAdapter = {
  push() {},
  replace() {},
  back() {},
  openInNewTab() {},
  pathname: "/card-preview",
  searchParams: new URLSearchParams(),
  hash: "",
  getShareableUrl: () => "#",
};
const client = new QueryClient({
  defaultOptions: { queries: { enabled: false, retry: false } },
});
client.setQueryData(workspaceKeys.list(), [workspace]);
client.setQueryData(workspaceKeys.members(workspace.id), []);
client.setQueryData(workspaceKeys.agents(workspace.id), [agent]);
client.setQueryData(workspaceKeys.teams(workspace.id), []);
client.setQueryData(propertyKeys.list(workspace.id), { properties: [] });
client.setQueryData(issueStatusKeys.list(workspace.id), { statuses: [] });
client.setQueryData(
  agentTaskSnapshotKeys.list(workspace.id),
  taskSnapshot("running"),
);
// Reuse the real state initializer without its persistence middleware.
const viewStore = createStore(viewStoreSlice);

function Preview() {
  const [status, setStatus] = useState<"in_progress" | "in_review">(
    "in_progress",
  );
  const [execution, setExecution] = useState<ExecutionState>("running");
  const [dark, setDark] = useState(false);
  useEffect(() => {
    document.documentElement.classList.toggle("dark", dark);
  }, [dark]);
  const [width, setWidth] = useState(340);
  return (
    <main className={dark ? "dark preview" : "preview"}>
      <div className="preview-shell">
        <p className="text-xs tracking-widest text-muted-foreground">
          PATCHBAY / LOCAL PREVIEW
        </p>
        <h1 className="mt-3 text-2xl font-semibold">卡片状态预览</h1>
        <p className="mt-2 text-sm text-muted-foreground">
          正式组件 · 内存数据 · 不连接后端、不启动 Agent
        </p>
        <section className="preview-controls" aria-label="预览控制">
          <label htmlFor="issue-status">
            任务状态
            <select
              id="issue-status"
              aria-label="任务状态"
              value={status}
              onChange={(e) => setStatus(e.target.value as typeof status)}
            >
              <option value="in_progress">In progress</option>
              <option value="in_review">In review</option>
            </select>
          </label>
          <label htmlFor="execution-state">
            模拟执行状态
            <select
              id="execution-state"
              aria-label="模拟执行状态"
              value={execution}
              onChange={(e) => {
                const value = e.target.value as ExecutionState;
                setExecution(value);
                client.setQueryData(
                  agentTaskSnapshotKeys.list(workspace.id),
                  taskSnapshot(value),
                );
              }}
            >
              <option value="idle">Idle · 没有任务</option>
              <option value="queued">Queued · 排队</option>
              <option value="running">Running · 运行</option>
            </select>
          </label>
          <label htmlFor="theme">
            主题
            <select
              id="theme"
              aria-label="主题"
              value={dark ? "dark" : "light"}
              onChange={(e) => setDark(e.target.value === "dark")}
            >
              <option value="light">Light</option>
              <option value="dark">Dark</option>
            </select>
          </label>
          <label>
            卡片宽度 · {width}px
            <input
              type="range"
              min="260"
              max="460"
              value={width}
              onChange={(e) => setWidth(Number(e.target.value))}
            />
          </label>
        </section>
        <section className="preview-canvas" aria-label="正式卡片">
          <div style={{ width, maxWidth: "100%" }}>
            <h2 className="mb-3 text-sm font-medium">
              {status === "in_progress" ? "In progress" : "In review"}
            </h2>
            <div className="group/card" data-testid="preview-card">
              <BoardCardContent issue={{ ...issue, status }} editable={false} />
            </div>
          </div>
        </section>
        <p className="mt-5 text-xs leading-6 text-muted-foreground">
          任务状态与执行状态独立。Running 使用正式卡片现有的 Working
          文字流光；Idle 不显示运行标记。
          <br />
          修改共享组件后刷新此页。此入口不验证真实任务调度，不能替代 staging
          联调。
        </p>
      </div>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <QueryClientProvider client={client}>
    <I18nProvider locale="en" resources={RESOURCES}>
      <WorkspaceSlugProvider slug={workspace.slug}>
        <NavigationProvider value={navigation}>
          <ViewStoreProvider store={viewStore}>
            <Preview />
          </ViewStoreProvider>
        </NavigationProvider>
      </WorkspaceSlugProvider>
    </I18nProvider>
  </QueryClientProvider>,
);
