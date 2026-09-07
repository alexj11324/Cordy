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

  it("activates workspace mode before creating Guest credentials", async () => {
    const ipc = bridge();
    ipc.enableCloudMode.mockImplementation(async () => {
      expect(localStorage.getItem("orvilo_token")).toBeNull();
      return { ok: true };
    });
    await startWorkspaceGuest({
      create: async () => ({ token: "guest-token", user: { is_guest: true } }),
      storage: localStorage, bridge: ipc,
    });
    expect(ipc.switchGuestToCloud).not.toHaveBeenCalled();
    expect(localStorage.getItem("orvilo_token")).toBe("guest-token");
  });

  it("restores existing credentials if the mode transition fails", async () => {
    localStorage.setItem("orvilo_token", "previous");
    const ipc = bridge();
    const create = vi.fn();
    ipc.enableCloudMode.mockResolvedValue({ ok: false, reason: "unavailable" });
    await expect(startWorkspaceGuest({
      create,
      storage: localStorage, bridge: ipc,
    })).rejects.toThrow("unavailable");
    expect(localStorage.getItem("orvilo_token")).toBe("previous");
    expect(create).not.toHaveBeenCalled();
  });

  it("does not enter the workspace when guest creation fails", async () => {
    const ipc = bridge();
    await expect(startWorkspaceGuest({
      create: async () => { throw new Error("offline"); },
      storage: localStorage, bridge: ipc,
    })).rejects.toThrow("offline");
    expect(ipc.enableCloudMode).toHaveBeenCalledOnce();
    expect(localStorage.getItem("orvilo_token")).toBeNull();
  });
});
