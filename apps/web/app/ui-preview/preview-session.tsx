"use client";

import { useLayoutEffect, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useAuthStore } from "@patchbay/core/auth";
import { workspaceKeys } from "@patchbay/core/workspace";
import type { User, Workspace } from "@patchbay/core/types";

const PREVIEW_WORKSPACE: Workspace = {
  id: "ws-preview",
  name: "Preview",
  slug: "preview",
  description: null,
  context: null,
  settings: {},
  repos: [],
  issue_prefix: "PRE",
  avatar_url: null,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
};

function previewUser(onboarded: boolean): User {
  return {
    id: "user-preview",
    name: "Preview",
    email: "preview@local",
    avatar_url: null,
    onboarded_at: onboarded ? "2026-01-01T00:00:00Z" : null,
    onboarding_questionnaire: {},
    starter_content_state: null,
    language: null,
    profile_description: "",
    timezone: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  };
}

/** Local-only session so onboarding and the app shell render without login. */
export function PreviewSession({
  onboarded,
  children,
}: {
  onboarded: boolean;
  children: ReactNode;
}) {
  const queryClient = useQueryClient();
  const user = useAuthStore((state) => state.user);

  useLayoutEffect(() => {
    queryClient.setQueryData(workspaceKeys.list(), [PREVIEW_WORKSPACE]);
    useAuthStore.setState({
      user: previewUser(onboarded),
      isLoading: false,
      status: "authenticated",
    });
  }, [onboarded, queryClient]);

  if (user?.id !== "user-preview") return null;
  return <>{children}</>;
}
