// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nProvider } from "@patchbay/core/i18n/react";
import { useModelFavoritesStore } from "@patchbay/core/agents/stores";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import enAgents from "../../locales/en/agents.json";
import enCommon from "../../locales/en/common.json";
import {
  ModelSelectorContent,
  type ModelSelectorRuntime,
} from "./model-selector-content";

const runtimes: ModelSelectorRuntime[] = [
  { id: "codex", name: "Codex Personal", provider: "codex", status: "online" },
  { id: "claude", name: "Claude Work", provider: "claude", status: "online" },
];
const catalogs = {
  qwenpaw: [],
  codex: [
    {
      id: "gpt",
      label: "GPT",
      thinking: {
        supported_levels: [
          { value: "low", label: "Low" },
          { value: "xhigh", label: "Extra high" },
        ],
      },
    },
  ],
  agy: [
    { id: "gemini-high", label: "Gemini (High)" },
    { id: "gemini-low", label: "Gemini (Low)" },
  ],
  claude: [
    {
      id: "opus",
      label: "Opus",
      thinking: { supported_levels: [{ value: "high", label: "High" }] },
    },
  ],
};
vi.mock("@patchbay/core/runtimes", () => ({
  runtimeModelsOptions: (id: keyof typeof catalogs | null) => ({
    queryKey: ["models", id],
    enabled: Boolean(id),
    queryFn: async () => ({
      supported: id !== "qwenpaw",
      models: id ? catalogs[id] : [],
    }),
  }),
}));
function mount(onSelect = vi.fn(), options = runtimes) {
  render(
    <I18nProvider
      locale="en"
      resources={{ en: { agents: enAgents, common: enCommon } }}
    >
      <QueryClientProvider
        client={
          new QueryClient({ defaultOptions: { queries: { retry: false } } })
        }
      >
        <ModelSelectorContent
          runtimes={options}
          runtimeId="codex"
          model="gpt"
          thinkingLevel="low"
          onSelect={onSelect}
        />
      </QueryClientProvider>
    </I18nProvider>,
  );
  return onSelect;
}
beforeEach(() => useModelFavoritesStore.setState({ favorites: [] }));
afterEach(cleanup);
describe("three-column model selector", () => {
  it("finishes a visible refresh revolution after an immediate cached response", async () => {
    mount();
    await screen.findByText("GPT");
    const refresh = screen.getByRole("button", {
      name: enAgents.model_selector.refresh,
    });
    await waitFor(() =>
      expect(refresh.getAttribute("aria-busy")).toBe("false"),
    );
    await userEvent.click(refresh);
    await waitFor(() =>
      expect(refresh.getAttribute("aria-busy")).toBe("false"),
    );
    const icon = refresh.querySelector("svg")!;
    expect(icon.classList.contains("animate-spin")).toBe(true);
    // jsdom lacks AnimationEvent; React registers its WebKit fallback.
    fireEvent(icon, new Event("webkitAnimationIteration", { bubbles: true }));
    expect(icon.classList.contains("animate-spin")).toBe(false);
  });
  it("can switch to a provider that manages its own model", async () => {
    const onSelect = mount(vi.fn(), [
      ...runtimes,
      { id: "qwenpaw", name: "QwenPaw", provider: "qwenpaw", status: "online" },
    ]);
    fireEvent.click(screen.getByRole("button", { name: "QwenPaw · qwenpaw" }));
    await screen.findByText(enAgents.pickers.model_managed_by_runtime);
    fireEvent.click(
      screen.getByRole("button", { name: enAgents.pickers.model_default }),
    );
    await waitFor(() =>
      expect(onSelect).toHaveBeenCalledWith({
        runtimeId: "qwenpaw",
        model: "",
        thinkingLevel: "",
        catalog: [],
      }),
    );
  });
  it("moves Antigravity native effort variants into the third column and saves the native model ID", async () => {
    const onSelect = mount(vi.fn(), [
      ...runtimes,
      {
        id: "agy",
        name: "Antigravity",
        provider: "antigravity",
        status: "online",
      },
    ]);
    fireEvent.click(
      screen.getByRole("button", { name: "Antigravity · antigravity" }),
    );
    fireEvent.click(await screen.findByText("Gemini"));
    expect(screen.queryByText("Gemini (High)")).toBeNull();
    fireEvent.click(
      screen.getByRole("button", { name: "Favorite Gemini (Low)" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Low" }));
    await waitFor(() =>
      expect(onSelect).toHaveBeenCalledWith({
        runtimeId: "agy",
        model: "gemini-low",
        thinkingLevel: "",
        catalog: catalogs.agy,
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Favorites" }));
    expect(screen.getByText("Gemini (Low)")).toBeTruthy();
  });
  it("browses provider and model without saving, then submits the exact effort with runtime and model", async () => {
    const onSelect = mount();
    fireEvent.click(
      screen.getByRole("button", { name: "Claude Work · claude" }),
    );
    fireEvent.click(await screen.findByText("Opus"));
    expect(onSelect).not.toHaveBeenCalled();
    expect(screen.queryByText("Extra high")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "High" }));
    await waitFor(() =>
      expect(onSelect).toHaveBeenCalledWith({
        runtimeId: "claude",
        model: "opus",
        thinkingLevel: "high",
        catalog: catalogs.claude,
      }),
    );
  });
  it("stars effort combinations independently and restores a favorite with one click", async () => {
    const onSelect = mount();
    fireEvent.click(
      await screen.findByRole("button", { name: "Favorite GPT (Low)" }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Favorite GPT (Extra high)" }),
    );
    expect(useModelFavoritesStore.getState().favorites).toHaveLength(2);
    expect(onSelect).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Favorites" }));
    fireEvent.click(screen.getByText("GPT (Extra high)"));
    await waitFor(() =>
      expect(onSelect).toHaveBeenCalledWith({
        runtimeId: "codex",
        model: "gpt",
        thinkingLevel: "xhigh",
        catalog: catalogs.codex,
      }),
    );
  });
  it("rejects a removed effort in a saved favorite instead of silently running a different one", async () => {
    useModelFavoritesStore.setState({
      favorites: [{ runtimeId: "codex", model: "gpt", thinkingLevel: "ultra" }],
    });
    const onSelect = mount();
    fireEvent.click(screen.getByText("gpt (ultra)"));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      enAgents.model_selector.favorite_unavailable,
    );
    expect(onSelect).not.toHaveBeenCalled();
  });
  it("keeps save failures visible and prevents duplicate writes while saving", async () => {
    let reject!: (error: Error) => void;
    const onSelect = vi.fn(
      () =>
        new Promise<void>((_, fail) => {
          reject = fail;
        }),
    );
    mount(onSelect);
    const high = await screen.findByRole("button", {
      name: "Extra high",
    });
    fireEvent.click(high);
    fireEvent.click(high);
    expect(onSelect).toHaveBeenCalledTimes(1);
    reject(new Error("Save failed"));
    expect(await screen.findByRole("alert")).toHaveTextContent("Save failed");
  });
  it("does not expose favorites from inaccessible runtimes", () => {
    useModelFavoritesStore.setState({
      favorites: [
        { runtimeId: "claude", model: "opus", thinkingLevel: "high" },
      ],
    });
    mount(vi.fn(), [runtimes[0]!]);
    fireEvent.click(screen.getByRole("button", { name: "Favorites" }));
    expect(screen.queryByText("opus (high)")).toBeNull();
    expect(
      screen.getByText(enAgents.model_selector.favorites_empty),
    ).toBeTruthy();
  });
});
