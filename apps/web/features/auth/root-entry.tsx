"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";
import { useAuthStore } from "@patchbay/core/auth";
import {
  paths,
  resolvePostAuthDestination,
  useHasOnboarded,
} from "@patchbay/core/paths";
import { useWorkspaceList } from "@patchbay/core/workspace";

/**
 * Root is an application entry point, not a marketing page.
 *
 * The proxy can immediately route a returning user when its last-workspace
 * cookie is present. This client fallback owns the two states that cannot be
 * resolved from cookies: a signed-out visitor goes to login, while a newly
 * authenticated visitor waits for the authoritative workspace list before
 * choosing onboarding, their first workspace, or workspace creation.
 */
export function RootEntry() {
  const router = useRouter();
  const user = useAuthStore((state) => state.user);
  const isLoading = useAuthStore((state) => state.isLoading);
  const hasOnboarded = useHasOnboarded();
  const { workspaces, ready } = useWorkspaceList({ enabled: !!user });

  useEffect(() => {
    if (isLoading) return;
    if (!user) {
      router.replace(paths.login());
      return;
    }
    if (!ready) return;
    router.replace(resolvePostAuthDestination(workspaces, hasOnboarded));
  }, [hasOnboarded, isLoading, ready, router, user, workspaces]);

  return null;
}
