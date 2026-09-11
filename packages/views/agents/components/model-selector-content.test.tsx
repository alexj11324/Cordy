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
import { I18nProvider } from "@orvilo/core/i18n/react";
import { useModelFavoritesStore } from "@orvilo/core/agents/stores";
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
const aliasedRuntimes: ModelSelectorRuntime[] = [
  {
    id: "codex",
    name: "Codex (host)",
    custom_name: "Primary Mac",
    provider: "codex",
    status: "online",
  },
  {
    id: "codex-build",
    name: "Codex (host)",
    custom_name: "Build Mac",
    provider: "codex",
    status: "online",
  },
];
const catalogs = {
  qwenpaw: [],
  codex: [
    {
      id: "gpt",
      label: "GPT",
      supports_explicit_standard_service_tier: true,
      service_tiers: [
        { id: "priority", name: "Fast", description: "Faster responses" },
      ],
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
    {
      id: "claude-opus-5",
      label: "Claude Opus 5",
      supports_explicit_standard_service_tier: true,
      service_tiers: [{ id: "true", name: "Fast" }],
      thinking: { supported_levels: [{ value: "high", label: "High" }] },
    },
    {
      id: "claude-sonnet-5",
      label: "Claude Sonnet 5",
      thinking: { supported_levels: [{ value: "low", label: "Low" }] },
    },
  ],
};
vi.mock("@orvilo/core/runtimes", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("@orvilo/core/runtimes")>();
  return {
    ...actual,
    runtimeModelsOptions: (id: keyof typeof catalogs | null) => ({
      queryKey: ["models", id],
      enabled: Boolean(id),
      queryFn: async () => ({
        supported: id !== "qwenpaw",
        models: id ? catalogs[id] : [],
      }),
    }),
  };
});
function mount(
  onSelect = vi.fn(),
  options = runtimes,
  props: Partial<React.ComponentProps<typeof ModelSelectorContent>> = {},
) {
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
          {...props}
        />
      </QueryClientProvider>
    </I18nProvider>,
  );
  return onSelect;
}
beforeEach(() => useModelFavoritesStore.setState({ favorites: [] }));
afterEach(cleanup);
describe("four-column model selector", () => {
  it("keeps model, effort, and speed columns from collapsing into each other", async () => {
    mount();
    await screen.findByRole("navigation", {
      name: enAgents.model_selector.providers,
    });
    const grid = document.querySelector("[data-model-selector] .grid");
    expect(grid?.className).toMatch(
      /grid-cols-\[minmax\(12\.5rem,1\.4fr\)_minmax\(10rem,1fr\)_minmax\(10\.5rem,1fr\)\]/,
    );
    expect(grid?.className).not.toMatch(/minmax\(0,/);
    expect(grid?.className).toMatch(/\boverflow-x-auto\b/);
  });

  it("matches T3 Code's provider rail: 44px column, 20px glyphs", async () => {
    mount();
    const nav = await screen.findByRole("navigation", {
      name: enAgents.model_selector.providers,
    });
    expect(nav.className).toMatch(/\bw-11\b/);
    const favorites = screen.getByRole("button", {
      name: enAgents.model_selector.favorites,
    });
    expect(favorites.querySelector("svg")?.getAttribute("class")).toMatch(
      /\bsize-5\b/,
    );
    const codex = screen.getByRole("button", {
      name: "Codex Personal",
    });
    expect(codex.querySelector("img, svg")?.getAttribute("class")).toMatch(
      /\bsize-5\b/,
    );
  });

  it("uses each runtime custom name in rail identity, selected title, and favorites", async () => {
    useModelFavoritesStore.setState({
      favorites: [{ runtimeId: "codex", model: "gpt", thinkingLevel: "low" }],
    });
    mount(vi.fn(), aliasedRuntimes, {
      preferFavorites: false,
      runtimeId: "codex",
      model: "gpt",
      thinkingLevel: "low",
    });

    const primaryRail = screen.getByRole("button", { name: "Primary Mac (Codex)" });
    const buildRail = screen.getByRole("button", { name: "Build Mac (Codex)" });
    expect(primaryRail).toHaveAttribute("title", "Primary Mac (Codex)");
    expect(buildRail).toHaveAttribute("title", "Build Mac (Codex)");
    expect(screen.getByText("Primary Mac (Codex)")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Favorites" }));
    expect(screen.getByText("Primary Mac (Codex)")).toBeInTheDocument();
  });

  it("keeps the current effort for a same-runtime custom model missing from a successful catalog", async () => {
    const onSelect = mount(vi.fn(), runtimes, {
      model: "gpt",
      thinkingLevel: "high",
    });
    await screen.findByText("GPT");
    const input = screen.getByPlaceholderText(
      enAgents.pickers.model_search_placeholder,
    );
    fireEvent.change(input, { target: { value: "custom-local-build" } });
    fireEvent.click(await screen.findByText('Use "custom-local-build"'));

    await waitFor(() =>
      expect(onSelect).toHaveBeenCalledWith(
        expect.objectContaining({
          runtimeId: "codex",
          model: "custom-local-build",
          thinkingLevel: "high",
        }),
      ),
    );
  });

  it("keeps the current effort for a same-runtime custom model while offline", async () => {
    const offlineRuntime: ModelSelectorRuntime = {
      ...runtimes[0]!,
      id: "codex-offline",
      status: "offline",
    };
    const onSelect = mount(vi.fn(), [offlineRuntime], {
      runtimeId: "codex-offline",
      model: "existing-model",
      thinkingLevel: "high",
      preferFavorites: false,
    });
    const input = screen.getByPlaceholderText(
      enAgents.pickers.model_search_placeholder,
    );
    fireEvent.change(input, { target: { value: "offline-custom-model" } });
    fireEvent.click(await screen.findByText('Use "offline-custom-model"'));

    await waitFor(() =>
      expect(onSelect).toHaveBeenCalledWith(
        expect.objectContaining({
          runtimeId: "codex-offline",
          model: "offline-custom-model",
          thinkingLevel: "high",
          catalog: null,
        }),
      ),
    );
  });

  it("clears the effort for a custom model when browsing a different runtime", async () => {
    const onSelect = mount(vi.fn(), runtimes);
    fireEvent.click(
      screen.getByRole("button", { name: "Claude Work" }),
    );
    await screen.findByText("Opus");
    const input = screen.getByPlaceholderText(
      enAgents.pickers.model_search_placeholder,
    );
    fireEvent.change(input, { target: { value: "claude-custom-model" } });
    fireEvent.click(await screen.findByText('Use "claude-custom-model"'));

    await waitFor(() =>
      expect(onSelect).toHaveBeenCalledWith(
        expect.objectContaining({
          runtimeId: "claude",
          model: "claude-custom-model",
          thinkingLevel: "",
        }),
      ),
    );
  });
  it("keeps a stale saved speed visible until the user explicitly clears it", async () => {
    const onSelect = mount(vi.fn(), runtimes, { serviceTier: "retired-fast" });
    await screen.findByText("GPT");
    expect(screen.getByText("retired-fast")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Remove unsupported speed retired-fast" }));
    await waitFor(() =>
      expect(onSelect).toHaveBeenCalledWith(
        expect.objectContaining({
          model: "gpt",
          thinkingLevel: "low",
          serviceTier: "",
        }),
      ),
    );
  });
  it("saves speed with the currently selected model and effort", async () => {
    const onSelect = mount();
    fireEvent.click(await screen.findByRole("button", { name: /^Fast/ }));
    await waitFor(() =>
      expect(onSelect).toHaveBeenCalledWith({
        runtimeId: "codex",
        model: "gpt",
        thinkingLevel: "low",
        serviceTier: "priority",
        catalog: catalogs.codex,
      }),
    );
  });
  it("preserves supported speed when changing thinking effort", async () => {
    const onSelect = mount(vi.fn(), runtimes, { serviceTier: "priority" });
    fireEvent.click(await screen.findByRole("button", { name: "Extra high" }));
    await waitFor(() =>
      expect(onSelect).toHaveBeenCalledWith(
        expect.objectContaining({
          model: "gpt",
          thinkingLevel: "xhigh",
          serviceTier: "priority",
        }),
      ),
    );
  });
  it("clears speed when switching to a different runtime", async () => {
    const onSelect = mount(vi.fn(), runtimes, { serviceTier: "priority" });
    fireEvent.click(
      screen.getByRole("button", { name: "Claude Work" }),
    );
    fireEvent.click(await screen.findByText("Opus"));
    fireEvent.click(screen.getByRole("button", { name: "High" }));
    await waitFor(() =>
      expect(onSelect).toHaveBeenCalledWith(
        expect.objectContaining({ runtimeId: "claude", serviceTier: "" }),
      ),
    );
    expect(screen.queryByRole("button", { name: /^Fast/ })).toBeNull();
    expect(screen.getByText(enAgents.model_selector.no_speed)).toBeTruthy();
  });
  it("scopes standard speed to the model that advertised it, not the runtime catalog", async () => {
    mount(vi.fn(), runtimes, { runtimeId: "claude", model: "opus" });
    fireEvent.click(await screen.findByText("Claude Opus 5"));
    expect(screen.getByRole("button", { name: /^Standard/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /^Fast/ })).toBeTruthy();
    fireEvent.click(screen.getByText("Claude Sonnet 5"));
    expect(screen.queryByRole("button", { name: /^Standard/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Fast/ })).toBeNull();
    expect(screen.getByText(enAgents.model_selector.no_speed)).toBeTruthy();
  });
  it("offers explicit standard independently from the runtime default", async () => {
    const onSelect = mount();
    fireEvent.click(await screen.findByRole("button", { name: /^Standard/ }));
    await waitFor(() =>
      expect(onSelect).toHaveBeenCalledWith(
        expect.objectContaining({ serviceTier: "default" }),
      ),
    );
  });
  it("shows inherited speed without writable options on model-only entries", async () => {
    mount(vi.fn(), runtimes, {
      allowEffort: false,
      allowSpeed: false,
      serviceTier: "priority",
    });
    await screen.findByText("GPT");
    expect(
      screen.getByText(enAgents.model_selector.speed_inherited),
    ).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^Fast/ })).toBeNull();
  });
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
  it("explains a host-managed runtime instead of offering a default model", async () => {
    const onSelect = mount(vi.fn(), [
      ...runtimes,
      { id: "qwenpaw", name: "QwenPaw", provider: "qwenpaw", status: "online" },
    ]);
    fireEvent.click(screen.getByRole("button", { name: "QwenPaw" }));
    await screen.findByText(enAgents.model_dropdown.managed_by_runtime_title);
    expect(
      screen.queryByRole("button", {
        name: enAgents.pickers.model_managed_by_runtime,
      }),
    ).toBeNull();
    fireEvent.click(
      screen.getByRole("button", {
        name: enAgents.pickers.bind_host_managed_runtime,
      }),
    );
    await waitFor(() =>
      expect(onSelect).toHaveBeenCalledWith({
        runtimeId: "qwenpaw",
        model: "",
        thinkingLevel: "",
        serviceTier: "",
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
      screen.getByRole("button", { name: "Antigravity" }),
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
        serviceTier: "",
        catalog: catalogs.agy,
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Favorites" }));
    expect(screen.getByText("Gemini (Low)")).toBeTruthy();
  });
  it("browses provider and model without saving, then submits the exact effort with runtime and model", async () => {
    const onSelect = mount();
    fireEvent.click(
      screen.getByRole("button", { name: "Claude Work" }),
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
        serviceTier: "",
        catalog: catalogs.claude,
      }),
    );
  });
  it("stars effort combinations independently and restores a favorite with one click", async () => {
    const onSelect = mount();
    fireEvent.click(
      await screen.findByRole("button", { name: "Favorite GPT (Low · Standard)" }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Favorite GPT (Extra high · Standard)" }),
    );
    expect(useModelFavoritesStore.getState().favorites).toHaveLength(2);
    expect(onSelect).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Favorites" }));
    fireEvent.click(screen.getByText("GPT (Extra high · Standard)"));
    await waitFor(() =>
      expect(onSelect).toHaveBeenCalledWith({
        runtimeId: "codex",
        model: "gpt",
        thinkingLevel: "xhigh",
        serviceTier: "default",
        catalog: catalogs.codex,
      }),
    );
  });
  it("shows saved speed on favorites and restores that exact tier", async () => {
    const onSelect = mount(vi.fn(), runtimes, { serviceTier: "priority" });
    fireEvent.click(
      await screen.findByRole("button", { name: "Favorite GPT (Low · Fast)" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Favorites" }));
    expect(screen.getByText("GPT (Low · Fast)")).toBeTruthy();
    fireEvent.click(screen.getByText("GPT (Low · Fast)"));
    await waitFor(() =>
      expect(onSelect).toHaveBeenCalledWith({
        runtimeId: "codex",
        model: "gpt",
        thinkingLevel: "low",
        serviceTier: "priority",
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

  it("keeps favorites search enabled when the bound runtime is inaccessible", () => {
    useModelFavoritesStore.setState({
      favorites: [
        { runtimeId: "claude", model: "opus", thinkingLevel: "high" },
      ],
    });
    mount(vi.fn(), [
      { ...runtimes[0]!, selectable: false },
      runtimes[1]!,
    ]);

    expect(
      screen.getByRole("textbox", {
        name: enAgents.pickers.model_search_placeholder,
      }),
    ).toBeEnabled();
  });
});
