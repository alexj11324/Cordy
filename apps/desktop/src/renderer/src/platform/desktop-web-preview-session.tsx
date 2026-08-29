import { useLayoutEffect, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useAuthStore } from "@patchbay/core/auth";
import { chatKeys } from "@patchbay/core/chat/queries";
import { inboxKeys } from "@patchbay/core/inbox";
import { pinKeys } from "@patchbay/core/pins";
import { runtimeKeys } from "@patchbay/core/runtimes";
import { workspaceKeys } from "@patchbay/core/workspace";
import type { User, Workspace } from "@patchbay/core/types";
import { isDesktopWebPreview } from "./web-bridge";

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

const PREVIEW_USER_ID = "user-preview";

function previewUser(onboarded: boolean): User {
  return {
    id: PREVIEW_USER_ID,
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

/**
 * Seeds the same local-only session that the old Next preview route used.
 * This component is a no-op in Electron and is only rendered as a preview
 * session when the Vite browser bridge explicitly enables it.
 */
export function DesktopWebPreviewSession({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient();
  const preview = isDesktopWebPreview();
  const onboarded =
    typeof window === "undefined" ||
    !window.location.pathname.startsWith("/ui-preview/onboarding");
  const user = useAuthStore((state) => state.user);

  useLayoutEffect(() => {
    if (!preview) return;

    // These defaults keep optional Desktop chrome from repeatedly retrying
    // endpoints while the issues board is being designed without a backend.
    queryClient.setQueryDefaults(["workspaces"], {
      staleTime: Infinity,
      retry: false,
    });
    queryClient.setQueryDefaults(["inbox"], {
      staleTime: Infinity,
      retry: false,
    });
    queryClient.setQueryDefaults(["pins"], {
      staleTime: Infinity,
      retry: false,
    });
    queryClient.setQueryDefaults(["chat"], {
      staleTime: Infinity,
      retry: false,
    });
    queryClient.setQueryDefaults(["runtimes"], {
      staleTime: Infinity,
      retry: false,
    });

    queryClient.setQueryData(workspaceKeys.list(), [PREVIEW_WORKSPACE]);
    queryClient.setQueryData(workspaceKeys.myInvitations(), []);
    queryClient.setQueryData(inboxKeys.list(PREVIEW_WORKSPACE.id), []);
    queryClient.setQueryData(inboxKeys.unreadSummary(), []);
    queryClient.setQueryData(chatKeys.sessions(PREVIEW_WORKSPACE.id), []);
    queryClient.setQueryData(
      chatKeys.pendingTasks(PREVIEW_WORKSPACE.id),
      { tasks: [] },
    );
    queryClient.setQueryData(
      chatKeys.pendingTasksHasAny(PREVIEW_WORKSPACE.id),
      { has_pending: false },
    );
    queryClient.setQueryData(
      pinKeys.list(PREVIEW_WORKSPACE.id, PREVIEW_USER_ID),
      [],
    );
    // Directory queries are served by the Vite local API below. Do not seed
    // empty arrays here: that would make React Query treat the directory as
    // resolved and prevent the shared renderer from discovering preview
    // members/agents.
    queryClient.setQueryData(workspaceKeys.skills(PREVIEW_WORKSPACE.id), []);
    queryClient.setQueryData(runtimeKeys.list(PREVIEW_WORKSPACE.id), []);
    useAuthStore.setState({
      user: previewUser(onboarded),
      isLoading: false,
      status: "authenticated",
    });
  }, [onboarded, preview, queryClient]);

  if (!preview || user?.id !== PREVIEW_USER_ID) return null;
  return <>{children}</>;
}
