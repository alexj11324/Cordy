import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { I18nProvider } from "@patchbay/core/i18n/react";
import enCommon from "../locales/en/common.json";
import enOnboarding from "../locales/en/onboarding.json";

const mocks = vi.hoisted(() => ({
  completeOnboarding: vi.fn(),
  setWelcome: vi.fn(),
  workspace: { id: "ws-preview", name: "Preview", slug: "preview" },
}));

vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: (selector: (state: { user: unknown }) => unknown) =>
    selector({ user: { id: "u-1", onboarding_questionnaire: {} } }),
}));

vi.mock("@patchbay/core/workspace", () => ({
  useWorkspaceList: () => ({ workspaces: [], ready: true }),
}));

vi.mock("@patchbay/core/onboarding", async () => {
  const actual = await vi.importActual<Record<string, unknown>>(
    "@patchbay/core/onboarding",
  );
  return {
    ...actual,
    completeOnboarding: mocks.completeOnboarding,
    useBootstrapMika: () => ({ mutateAsync: vi.fn() }),
    useWelcomeStore: { getState: () => ({ set: mocks.setWelcome }) },
  };
});

vi.mock("./steps/step-workspace", () => ({
  StepWorkspace: ({
    onCreated,
  }: {
    onCreated: (workspace: typeof mocks.workspace) => void;
  }) => (
    <button type="button" onClick={() => onCreated(mocks.workspace)}>
      Create preview workspace
    </button>
  ),
}));

vi.mock("./steps/step-platform-fork", () => ({
  StepPlatformFork: ({ onNext }: { onNext: (runtime: null) => void }) => (
    <button type="button" onClick={() => onNext(null)}>
      Skip preview runtime
    </button>
  ),
}));

vi.mock("./steps/step-runtime-connect", () => ({
  StepRuntimeConnect: () => null,
}));

vi.mock("./steps/step-welcome", () => ({ StepWelcome: () => null }));
vi.mock("./components/onboarding-logout-button", () => ({
  OnboardingLogoutButton: () => null,
}));
vi.mock("./components/step-shell", () => ({
  StepShell: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

import { OnboardingFlow } from "./onboarding-flow";

function renderRuntimeSkip(
  backendFree: boolean,
  onComplete: ReturnType<typeof vi.fn>,
) {
  render(
    <I18nProvider
      locale="en"
      resources={{ en: { common: enCommon, onboarding: enOnboarding } }}
    >
      <OnboardingFlow
        mode="new_workspace"
        backendFree={backendFree}
        runtimeInstructions={<div>CLI instructions</div>}
        onComplete={
          onComplete as React.ComponentProps<typeof OnboardingFlow>["onComplete"]
        }
      />
    </I18nProvider>,
  );
  fireEvent.click(
    screen.getByRole("button", { name: "Create preview workspace" }),
  );
  fireEvent.click(screen.getByRole("button", { name: "Skip preview runtime" }));
}

describe("OnboardingFlow backend-free runtime skip", () => {
  beforeEach(() => {
    mocks.completeOnboarding.mockReset();
    mocks.completeOnboarding.mockResolvedValue(undefined);
    mocks.setWelcome.mockReset();
  });

  it("completes the Vite preview locally without calling the backend", async () => {
    const onComplete = vi.fn();

    renderRuntimeSkip(true, onComplete);

    await waitFor(() =>
      expect(onComplete).toHaveBeenCalledWith(mocks.workspace, undefined),
    );
    expect(mocks.completeOnboarding).not.toHaveBeenCalled();
    expect(mocks.setWelcome).toHaveBeenCalledWith({
      workspaceId: mocks.workspace.id,
      choice: "skip",
    });
  });

  it("still persists runtime skip when a backend is available", async () => {
    const onComplete = vi.fn();

    renderRuntimeSkip(false, onComplete);

    await waitFor(() =>
      expect(mocks.completeOnboarding).toHaveBeenCalledWith(
        "runtime_skipped",
        mocks.workspace.id,
      ),
    );
    expect(onComplete).toHaveBeenCalledWith(mocks.workspace, undefined);
  });
});
