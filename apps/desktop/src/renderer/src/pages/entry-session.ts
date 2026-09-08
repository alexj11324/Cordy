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
  // Activate local capabilities before allocating a server session. The entry
  // owner mounts CoreProvider only after this entire operation succeeds.
  await enableWorkspaceMode(bridge);
  const { token, user } = await create();
  if (!token || user.is_guest !== true) throw new Error("Invalid Guest session");
  storage.setItem("orvilo_token", token);
}
