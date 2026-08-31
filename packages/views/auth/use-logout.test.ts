// @vitest-environment jsdom
import { describe, expect, it, vi, beforeEach } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { useLogout } from "./use-logout";

// Order of the destructive calls is the contract under test: each in-memory
// reset is a Zustand setState, and persist middleware writes the reset state
// straight back to storage under the still-active workspace slug. Resetting
// AFTER the per-slug key removal therefore resurrects the just-deleted keys
// (for the issue draft store: with the previous user's lastExecutor inside).
const calls = vi.hoisted(() => [] as string[]);
const mockReset = vi.hoisted(() => vi.fn());
const mockClearWorkspaceStorage = vi.hoisted(() => vi.fn());
const mockAuthLogout = vi.hoisted(() => vi.fn());
const mockPush = vi.hoisted(() => vi.fn());
const mockQueryClientClear = vi.hoisted(() => vi.fn());

vi.mock("@tanstack/react-query", () => ({
  useQueryClient: () => ({
    getQueryData: () => [
      { slug: "acme" },
      { slug: "beta" },
    ],
    clear: mockQueryClientClear,
  }),
}));

vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: Object.assign(
    (selector?: (s: unknown) => unknown) => {
      const state = { logout: mockAuthLogout };
      return selector ? selector(state) : state;
    },
    { getState: () => ({ logout: mockAuthLogout }) },
  ),
}));

vi.mock("@patchbay/core/workspace/queries", () => ({
  workspaceKeys: { list: () => ["workspaces", "list"] },
}));

vi.mock("@patchbay/core/platform", () => ({
  clearWorkspaceStorage: mockClearWorkspaceStorage,
  defaultStorage: { getItem: () => null, setItem: () => {}, removeItem: () => {} },
}));

vi.mock("@patchbay/core/drafts/cleanup-registry", () => ({
  resetAllRegisteredDrafts: mockReset,
}));

vi.mock("@patchbay/core/paths", () => ({
  paths: { login: () => "/login" },
}));

vi.mock("../navigation", () => ({
  useNavigation: () => ({ push: mockPush }),
}));

describe("useLogout", () => {
  beforeEach(() => {
    calls.length = 0;
    vi.clearAllMocks();
    mockReset.mockImplementation(() => calls.push("reset"));
    mockClearWorkspaceStorage.mockImplementation((_a: unknown, slug: string) =>
      calls.push(`clear:${slug}`),
    );
    mockAuthLogout.mockResolvedValue(undefined);
  });

  it("resets in-memory drafts BEFORE removing their persisted keys", async () => {
    const { result } = renderHook(() => useLogout());
    await act(async () => result.current());

    expect(calls).toEqual(["reset", "clear:acme", "clear:beta"]);
  });

  it("still ends by clearing the query cache, auth, and navigating to /login", async () => {
    const { result } = renderHook(() => useLogout());
    await act(async () => result.current());

    expect(mockQueryClientClear).toHaveBeenCalledTimes(1);
    expect(mockAuthLogout).toHaveBeenCalledTimes(1);
    expect(mockPush).toHaveBeenCalledWith("/login");
  });

  it("waits for platform session revocation before navigating", async () => {
    let finishAuthLogout: (() => void) | undefined;
    mockAuthLogout.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          finishAuthLogout = resolve;
        }),
    );
    const { result } = renderHook(() => useLogout());

    let logout: Promise<void> | undefined;
    act(() => {
      logout = result.current();
    });
    expect(mockPush).not.toHaveBeenCalled();

    await act(async () => {
      finishAuthLogout?.();
      await logout;
    });
    expect(mockPush).toHaveBeenCalledWith("/login");
  });
});
