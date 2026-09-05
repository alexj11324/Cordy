import { beforeEach, describe, expect, it, vi } from "vitest";
import { enableWorkspaceMode, startWorkspaceGuest } from "./entry-session";

beforeEach(() => localStorage.clear());

function bridge() {
  return {
    enableCloudMode: vi.fn().mockResolvedValue({ ok: true }),
    switchGuestToCloud: vi.fn().mockResolvedValue({ ok: true }),
  };
}

describe("desktop workspace entry", () => {
  it("retires a legacy local Guest only after an explicit entry action", async () => {
    const ipc = bridge();
    ipc.enableCloudMode.mockResolvedValue({ ok: false, reason: "guest_active" });
    await enableWorkspaceMode(ipc);
    expect(ipc.switchGuestToCloud).toHaveBeenCalledOnce();
  });

  it("persists Guest credentials before main emits the workspace mode event", async () => {
    const ipc = bridge();
    ipc.enableCloudMode.mockImplementation(async () => {
      expect(localStorage.getItem("patchbay_token")).toBe("guest-token");
      return { ok: true };
    });
    await startWorkspaceGuest({
      create: async () => ({ token: "guest-token", user: { is_guest: true } }),
      storage: localStorage, bridge: ipc,
    });
    expect(ipc.switchGuestToCloud).not.toHaveBeenCalled();
  });

  it("restores existing credentials if the mode transition fails", async () => {
    localStorage.setItem("patchbay_token", "previous");
    const ipc = bridge();
    ipc.enableCloudMode.mockResolvedValue({ ok: false, reason: "unavailable" });
    await expect(startWorkspaceGuest({
      create: async () => ({ token: "guest-token", user: { is_guest: true } }),
      storage: localStorage, bridge: ipc,
    })).rejects.toThrow("unavailable");
    expect(localStorage.getItem("patchbay_token")).toBe("previous");
  });

  it("does not enter the workspace when guest creation fails", async () => {
    const ipc = bridge();
    await expect(startWorkspaceGuest({
      create: async () => { throw new Error("offline"); },
      storage: localStorage, bridge: ipc,
    })).rejects.toThrow("offline");
    expect(ipc.enableCloudMode).not.toHaveBeenCalled();
    expect(localStorage.getItem("patchbay_token")).toBeNull();
  });
});
