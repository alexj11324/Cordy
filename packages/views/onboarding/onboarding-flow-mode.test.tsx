import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { I18nProvider } from "@patchbay/core/i18n/react";
import enCommon from "../locales/en/common.json";
import enOnboarding from "../locales/en/onboarding.json";
import enWorkspace from "../locales/en/workspace.json";

const TEST_RESOURCES = {
  en: { common: enCommon, onboarding: enOnboarding, workspace: enWorkspace },
};

vi.mock("../auth", () => ({ useLogout: () => vi.fn() }));

vi.mock("@patchbay/core/config", () => ({
  useConfigStore: (
    selector: (s: { workspaceCreationDisabled: boolean; daemonAppUrl: string }) => unknown,
  ) => selector({ workspaceCreationDisabled: false, daemonAppUrl: "" }),
}));

vi.mock("@patchbay/core/api", () => ({
  api: { getBaseUrl: () => "https://patchbay.ai" },
}));

vi.mock("@patchbay/core/workspace/mutations", () => ({
  useCreateWorkspace: () => ({
    mutate: vi.fn(),
    mutateAsync: vi.fn(),
    isPending: false,
  }),
}));

vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: Object.assign(
    (selector: (s: { user: unknown }) => unknown) =>
      selector({ user: { id: "u-1", onboarding_questionnaire: {} } }),
    { getState: () => ({ user: { id: "u-1" } }) },
  ),
}));

// Returning one workspace proves new-workspace mode does not offer to
// continue with it.
vi.mock("@patchbay/core/workspace", () => {
  return {
    useWorkspaceList: () => ({
      workspaces: [{ id: "ws-1", name: "Existing", slug: "existing" }],
      ready: true,
    }),
  };
});

vi.mock("@patchbay/core/onboarding", async () => {
  const actual = await vi.importActual<Record<string, unknown>>(
    "@patchbay/core/onboarding",
  );
  return { ...actual, useBootstrapPatrick: () => ({ mutateAsync: vi.fn() }) };
});

import { OnboardingFlow } from "./onboarding-flow";

function renderFlow(props: Record<string, unknown>) {
  return render(
    <I18nProvider locale="en" resources={TEST_RESOURCES}>
      <OnboardingFlow onComplete={vi.fn()} {...props} />
    </I18nProvider>,
  );
}

describe("OnboardingFlow — new-workspace mode", () => {
  it("starts at the workspace step instead of the product intro", () => {
    renderFlow({ mode: "new_workspace", onCancel: vi.fn() });

    // The welcome screen teaches what Patchbay is; someone creating a second
    // workspace already knows, so the flow opens on naming it.
    expect(
      screen.getByRole("heading", { name: /Set up your first workspace/i }),
    ).toBeInTheDocument();
  });

  it("always creates a workspace rather than offering the existing one", () => {
    renderFlow({ mode: "new_workspace", onCancel: vi.fn() });

    // First-run onboarding offers "Continue with {name}" when an abandoned
    // workspace exists. Doing that here would defeat the whole intent.
    expect(screen.queryByText(/Continue with/i)).not.toBeInTheDocument();
    expect(screen.getByLabelText("Workspace name")).toBeInTheDocument();
  });

  it("still opens on the product intro in first-run mode", () => {
    renderFlow({});

    expect(
      screen.queryByRole("heading", { name: /Set up your first workspace/i }),
    ).not.toBeInTheDocument();
  });
});
