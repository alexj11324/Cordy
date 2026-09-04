import { render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const state = vi.hoisted(() => ({
  user: { id: "user-1" } as { id: string } | null,
  isLoading: false,
  hasOnboarded: true,
  workspaces: [] as { id: string; slug: string }[],
  ready: false,
  replace: vi.fn(),
}));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ replace: state.replace }),
}));

vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: (
    selector: (auth: {
      user: typeof state.user;
      isLoading: boolean;
    }) => unknown,
  ) => selector({ user: state.user, isLoading: state.isLoading }),
}));

vi.mock("@patchbay/core/workspace", () => ({
  useWorkspaceList: () => ({
    workspaces: state.workspaces,
    ready: state.ready,
  }),
}));

vi.mock("@patchbay/core/paths", () => ({
  paths: { login: () => "/login" },
  useHasOnboarded: () => state.hasOnboarded,
  resolvePostAuthDestination: (
    workspaces: { slug: string }[],
    hasOnboarded: boolean,
  ) => {
    if (!hasOnboarded) return "/onboarding";
    return workspaces[0] ? `/${workspaces[0].slug}/issues` : "/workspaces/new";
  },
}));

import { RootEntry } from "./root-entry";

beforeEach(() => {
  state.user = { id: "user-1" };
  state.isLoading = false;
  state.hasOnboarded = true;
  state.workspaces = [];
  state.ready = false;
  state.replace.mockReset();
});

describe("RootEntry", () => {
  it("routes a signed-out visitor to login after authentication settles", async () => {
    state.user = null;

    render(<RootEntry />);

    await waitFor(() => expect(state.replace).toHaveBeenCalledWith("/login"));
  });

  it("waits for an authoritative workspace list", () => {
    render(<RootEntry />);

    expect(state.replace).not.toHaveBeenCalled();
  });

  it("routes an authenticated visitor to their first workspace", async () => {
    state.ready = true;
    state.workspaces = [{ id: "ws-1", slug: "acme" }];

    render(<RootEntry />);

    await waitFor(() =>
      expect(state.replace).toHaveBeenCalledWith("/acme/issues"),
    );
  });

  it("routes a new authenticated visitor through onboarding", async () => {
    state.ready = true;
    state.hasOnboarded = false;

    render(<RootEntry />);

    await waitFor(() =>
      expect(state.replace).toHaveBeenCalledWith("/onboarding"),
    );
  });

  it("routes an onboarded visitor without workspaces to creation", async () => {
    state.ready = true;

    render(<RootEntry />);

    await waitFor(() =>
      expect(state.replace).toHaveBeenCalledWith("/workspaces/new"),
    );
  });
});
