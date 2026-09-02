import type { LocalGuestMode } from "./local-guest";
import {
  MAIN_RENDERER_MESSAGE_CHANNELS,
  type MainRendererMessageChannel,
} from "./main-renderer-messages";

export type MainRendererChannelScope = "cloud" | "local";

/**
 * Every main → renderer channel classified by the account data it can reach.
 *
 * This is written as a total `Record` over `MainRendererMessageChannel` on
 * purpose: adding a channel to `MAIN_RENDERER_MESSAGE_CHANNELS` without
 * classifying it here is a compile error. A new cloud deep link therefore
 * cannot reach a Guest renderer by omission — the failure mode this table
 * exists to prevent.
 */
export const MAIN_RENDERER_CHANNEL_SCOPES: Record<
  MainRendererMessageChannel,
  MainRendererChannelScope
> = {
  // The accounts-broker handoff — carries a cloud credential.
  "auth:handoff": "cloud",
  // patchbay://invite/<invitationId> — joins a cloud workspace.
  "invite:open": "cloud",
  // Native notification click — navigates to a cloud issue in a workspace tab.
  "inbox:open": "cloud",
  // Settings is a workspace tab that only exists inside the cloud shell.
  "settings:open": "cloud",
};

export const CLOUD_MAIN_RENDERER_CHANNELS: readonly MainRendererMessageChannel[] =
  MAIN_RENDERER_MESSAGE_CHANNELS.filter(
    (channel) => MAIN_RENDERER_CHANNEL_SCOPES[channel] === "cloud",
  );

/**
 * What main must do with a renderer-bound payload.
 *
 * - `deliver`: hand it to the renderer message queue.
 * - `reject`: drop it and clear anything already queued for cloud channels.
 * - `defer`: hold it until the main-owned mode is decided. An auth or invite
 *   deep link is itself an explicit cloud intent, so it must survive the boot
 *   race between "the URL arrived" and "the user picked a mode" — but it must
 *   never be delivered speculatively.
 */
export type DeepLinkDisposition = "deliver" | "reject" | "defer";

export function deepLinkDisposition(
  channel: MainRendererMessageChannel,
  mode: LocalGuestMode,
): DeepLinkDisposition {
  if (MAIN_RENDERER_CHANNEL_SCOPES[channel] === "local") return "deliver";
  switch (mode) {
    case "cloud":
      return "deliver";
    case "guest":
      return "reject";
    case "undecided":
      return "defer";
  }
}
