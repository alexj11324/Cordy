import type { GuestCloudModeResult } from "../../../shared/local-guest";

type ModeBridge = {
  enableCloudMode: () => Promise<GuestCloudModeResult>;
  switchGuestToCloud: () => Promise<GuestCloudModeResult>;
};

// Both entry actions use the regular workspace. Explicitly choosing one also
// retires a legacy local Guest marker, without deleting its directory or history.
export async function enableWorkspaceMode(bridge: ModeBridge): Promise<void> {
  let result = await bridge.enableCloudMode();
  if (!result.ok && result.reason === "guest_active") {
    result = await bridge.switchGuestToCloud();
  }
  if (!result.ok) throw new Error(`Unable to enable workspace: ${result.reason}`);
}

export async function startWorkspaceGuest({ create, storage, bridge }: {
  create: () => Promise<{ token: string; user: { is_guest?: boolean } }>;
  storage: Pick<Storage, "getItem" | "setItem" | "removeItem">;
  bridge: ModeBridge;
}): Promise<void> {
  const { token, user } = await create();
  if (!token || user.is_guest !== true) throw new Error("Invalid Guest session");
  const previous = storage.getItem("patchbay_token");
  // The IPC mode event can mount CoreProvider before the IPC promise resolves.
  storage.setItem("patchbay_token", token);
  try {
    await enableWorkspaceMode(bridge);
  } catch (error) {
    if (previous) storage.setItem("patchbay_token", previous);
    else storage.removeItem("patchbay_token");
    throw error;
  }
}
