import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { I18nProvider } from "@patchbay/core/i18n/react";
import { RESOURCES } from "@patchbay/views/locales";
import type { LocalRuntimeProbe } from "../../../shared/daemon-types";
import type { LocalGuestSession } from "../../../shared/local-guest";
import { LocalGuestShell } from "./local-guest";

const session: LocalGuestSession = { displayName: "Alice" };

function renderShell(probeLocalRuntimes: ReturnType<typeof vi.fn>) {
  const pickDirectory = vi.fn().mockResolvedValue({
    ok: true,
    path: "/home/alice/projects",
  });
  const validateLocalDirectory = vi.fn().mockResolvedValue({ ok: true });
  Object.defineProperty(window, "desktopAPI", {
    configurable: true,
    value: { pickDirectory, validateLocalDirectory, probeLocalRuntimes },
  });

  render(
    <I18nProvider locale="en" resources={RESOURCES}>
      <LocalGuestShell
        session={session}
        onSwitchToCloud={vi.fn().mockResolvedValue(undefined)}
        onExit={vi.fn().mockResolvedValue(undefined)}
      />
    </I18nProvider>,
  );

  return { pickDirectory, validateLocalDirectory };
}

beforeEach(() => {
  vi.restoreAllMocks();
});

describe("LocalGuestShell", () => {
  it("shows local identity, chooses a directory, and renders runtime inventory", async () => {
    const probe: LocalRuntimeProbe = {
      probeResult: "success",
      runtimeCount: 2,
      providerSummary: { claude: 1, codex: 1 },
      onlineCount: 0,
      offlineCount: 2,
    };
    const probeLocalRuntimes = vi.fn().mockResolvedValue(probe);
    const { pickDirectory, validateLocalDirectory } = renderShell(
      probeLocalRuntimes,
    );

    expect(await screen.findByText("Alice")).toBeInTheDocument();
    expect(await screen.findByText("claude")).toBeInTheDocument();
    expect(screen.getByText("2 runtimes")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Choose directory" }));
    await waitFor(() => {
      expect(pickDirectory).toHaveBeenCalledWith(undefined);
      expect(validateLocalDirectory).toHaveBeenCalledWith(
        "/home/alice/projects",
      );
    });
    expect(await screen.findByText("/home/alice/projects")).toBeInTheDocument();
  });

  it("renders a fail-closed message when the packaged runtime is unavailable", async () => {
    const probeLocalRuntimes = vi.fn().mockResolvedValue({
      probeResult: "error",
    } satisfies LocalRuntimeProbe);
    renderShell(probeLocalRuntimes);

    expect(
      await screen.findByText("The packaged local runtime is unavailable."),
    ).toBeInTheDocument();
    expect(probeLocalRuntimes).toHaveBeenCalledTimes(1);
  });
});
