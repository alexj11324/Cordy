"use client";

import { useCallback } from "react";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { useModalStore } from "@patchbay/core/modals";

/** Opens the shared issue-limit recovery dialog without closing the current draft. */
export function useIssueLimitUpgradePrompt(): () => void {
  const wsId = useWorkspaceId();

  return useCallback(() => {
    useModalStore.getState().showIssueLimitRecovery(wsId);
  }, [wsId]);
}
