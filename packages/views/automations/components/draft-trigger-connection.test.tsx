import { focusManager, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { renderWithI18n } from "../../test/i18n";
import { NavigationProvider } from "../../navigation";
import { DraftTriggerConnection } from "./draft-trigger-connection";

const mocks = vi.hoisted(() => ({ github: vi.fn(), slack: vi.fn(), linear: vi.fn(), push: vi.fn(), openInNewTab: vi.fn() }));
vi.mock("@orvilo/core/api", () => ({ api: {
  listGitHubInstallations: mocks.github,
  listSlackInstallations: mocks.slack,
  getLinearConnection: mocks.linear,
} }));
vi.mock("@orvilo/core/hooks", () => ({ useWorkspaceId: () => "ws-test" }));
vi.mock("@orvilo/core/paths", () => ({ useWorkspacePaths: () => ({ settings: () => "/acme/settings" }) }));

function createClient() {
  return new QueryClient({ defaultOptions: { queries: {
    retry: false,
    staleTime: Infinity,
    refetchOnWindowFocus: false,
  } } });
}

function renderConnection(provider: string, desktop = false, client = createClient()) {
  renderWithI18n(
    <QueryClientProvider client={client}>
      <NavigationProvider value={{
        pathname: "/acme/automations/new", searchParams: new URLSearchParams(), hash: "",
        push: mocks.push, replace: vi.fn(), back: vi.fn(),
        getShareableUrl: (path) => `https://example.test${path}`,
        ...(desktop ? { openInNewTab: mocks.openInNewTab } : {}),
      }}>
        <DraftTriggerConnection provider={provider} disabled={false} />
      </NavigationProvider>
    </QueryClientProvider>,
  );
  return client;
}

describe("Draft trigger provider connections", () => {
  beforeEach(() => {
    mocks.github.mockReset().mockResolvedValue({ installations: [] });
    mocks.slack.mockReset().mockResolvedValue({ installations: [] });
    mocks.linear.mockReset().mockResolvedValue({ connected: false });
    mocks.push.mockReset();
    mocks.openInNewTab.mockReset();
  });
  afterEach(() => focusManager.setFocused(undefined));

  it.each([
    ["github", "github"], ["slack", "integrations"], ["linear", "integrations"],
  ])("offers the real %s settings connection without leaving the Web draft", async (provider, tab) => {
    renderConnection(provider);
    const link = await screen.findByRole("button", { name: "Connect" });
    expect(link).toHaveAttribute("href", `/acme/settings?tab=${tab}`);
    expect(link).toHaveAttribute("target", "_blank");
    expect(link).toHaveAttribute("rel", "noopener noreferrer");
    await userEvent.click(link);
    expect(mocks.push).not.toHaveBeenCalled();
    expect(screen.getByText("Requires connection")).toBeInTheDocument();
    expect(mocks[provider as "github" | "slack" | "linear"]).toHaveBeenCalledWith("ws-test");
  });

  it("routes Desktop connection through its separate settings destination", async () => {
    renderConnection("github", true);
    await userEvent.click(await screen.findByRole("button", { name: "Connect" }));
    expect(mocks.openInNewTab).toHaveBeenCalledWith("/acme/settings?tab=github", undefined, { activate: true });
    expect(mocks.push).not.toHaveBeenCalled();
  });

  it.each(["github", "slack", "linear"])("does not offer Connect for connected %s", async (provider) => {
    mocks.github.mockResolvedValue({ installations: [{ id: "github-1" }] });
    mocks.slack.mockResolvedValue({ installations: [{ status: "installed" }] });
    mocks.linear.mockResolvedValue({ connected: true, connection: { status: "active" } });
    renderConnection(provider);
    await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument());
    expect(screen.queryByRole("button", { name: "Connect" })).not.toBeInTheDocument();
  });

  it("offers reconnection when Linear authorization is no longer active", async () => {
    mocks.linear.mockResolvedValue({ connected: true, connection: { status: "reauthorization_required" } });
    renderConnection("linear");
    expect(await screen.findByRole("button", { name: "Connect" })).toBeInTheDocument();
  });

  it.each(["github", "slack", "linear"])("refreshes %s after returning from connection settings", async (provider) => {
    renderConnection(provider);
    await screen.findByRole("button", { name: "Connect" });
    act(() => focusManager.setFocused(false));
    mocks.github.mockResolvedValue({ installations: [{ id: "github-1" }] });
    mocks.slack.mockResolvedValue({ installations: [{ status: "installed" }] });
    mocks.linear.mockResolvedValue({ connected: true, connection: { status: "active" } });
    act(() => focusManager.setFocused(true));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Connect" })).not.toBeInTheDocument());
    expect(mocks[provider as "github" | "slack" | "linear"]).toHaveBeenCalledTimes(2);
  });

  it("refreshes a cached disconnected Linear state when opening the draft", async () => {
    const client = createClient();
    client.setQueryData(["linear", "ws-test", "connection"], { connected: false });
    mocks.linear.mockResolvedValue({ connected: true, connection: { status: "active" } });
    renderConnection("linear", false, client);
    await waitFor(() => expect(mocks.linear).toHaveBeenCalledWith("ws-test"));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Connect" })).not.toBeInTheDocument());
  });

  it("shows pending connection status without falsely reporting disconnected", () => {
    mocks.github.mockImplementation(() => new Promise(() => {}));
    renderConnection("github");
    expect(screen.getByRole("status")).toHaveTextContent("Checking connection...");
    expect(screen.queryByRole("button", { name: "Connect" })).not.toBeInTheDocument();
  });

  it("allows retrying a failed connection lookup", async () => {
    mocks.linear.mockRejectedValueOnce(new Error("Network unavailable"));
    renderConnection("linear");
    expect(await screen.findByText("Could not check connection")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Connect" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("button", { name: "Connect" })).toBeInTheDocument();
    expect(mocks.linear).toHaveBeenCalledTimes(2);
  });

  it("does not offer an OAuth connection for a generic webhook", () => {
    renderConnection("generic");
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
    expect(mocks.github).not.toHaveBeenCalled();
    expect(mocks.slack).not.toHaveBeenCalled();
    expect(mocks.linear).not.toHaveBeenCalled();
  });
});
