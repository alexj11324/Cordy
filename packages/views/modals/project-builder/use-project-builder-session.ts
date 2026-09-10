"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, ApiError } from "@orvilo/core/api";
import {
  chatKeys,
  chatMessagesOptions,
  pendingChatTaskOptions,
} from "@orvilo/core/chat/queries";
import { upsertChatMessageToCaches } from "@orvilo/core/chat/message-cache";
import { removeChatMessageFromCaches } from "@orvilo/core/realtime";
import type { Agent, Attachment, ChatMessage } from "@orvilo/core/types";
import { useAppForeground } from "../../common/use-app-foreground";
import {
  useChatDraftRestore,
  type RestoreDraftRequest,
} from "../../chat/components/use-chat-draft-restore";
import { useT } from "../../i18n";
import {
  decodeProjectBuilderInput,
  encodeProjectBuilderInput,
  type ProjectBuilderCatalogs,
  type ProjectBuilderDraft,
} from "./project-builder-draft";

const EMPTY_CHAT_MESSAGES: ChatMessage[] = [];

/**
 * The project assistant uses an ordinary chat session bound to the selected
 * agent. The session is created lazily with the first send so choosing an
 * agent or opening the panel never creates a blank conversation.
 */
export function useProjectBuilderSession(options: {
  agent: Agent | null;
  draft: ProjectBuilderDraft;
  catalogs: ProjectBuilderCatalogs;
}) {
  const { agent, draft, catalogs } = options;
  const queryClient = useQueryClient();
  const appForeground = useAppForeground();
  const { t } = useT("modals");
  const mountedRef = useRef(false);
  const sessionIdRef = useRef("");
  const createSessionRequestRef = useRef<Promise<string | null> | null>(null);
  const destroyRequestRef = useRef<{
    sessionId: string;
    promise: Promise<boolean>;
  } | null>(null);

  const [sessionId, setSessionId] = useState("");
  const [starting, setStarting] = useState(false);
  const [closing, setClosing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [localRestore, setLocalRestore] = useState<RestoreDraftRequest | null>(null);

  const messagesQuery = useQuery(chatMessagesOptions(sessionId));
  const pendingQuery = useQuery(pendingChatTaskOptions(sessionId));
  const messages = messagesQuery.data ?? EMPTY_CHAT_MESSAGES;
  const pendingTask = pendingQuery.data;
  const pending = Boolean(pendingTask?.task_id);
  const missing =
    messagesQuery.error instanceof ApiError && messagesQuery.error.status === 404;

  const durableRestore = useChatDraftRestore(sessionId || null, appForeground);
  const restoreDraftRequest = useMemo(() => {
    const restore = localRestore ?? durableRestore.restoreDraftRequest;
    if (!restore) return null;
    return {
      ...restore,
      content: decodeProjectBuilderInput(restore.content),
    };
  }, [durableRestore.restoreDraftRequest, localRestore]);

  const ensureSession = useCallback(async (): Promise<string | null> => {
    const existing = sessionIdRef.current;
    if (existing) return existing;
    if (!agent) return null;
    if (createSessionRequestRef.current) return createSessionRequestRef.current;

    setStarting(true);
    setError(null);
    const request = api
      .createChatSession({
        agent_id: agent.id,
        title: "Create project",
      })
      .then((session) => {
        if (!session.id) {
          throw new Error("The chat session response did not include an id");
        }
        sessionIdRef.current = session.id;
        return session.id;
      })
      .catch((cause) => {
        setError(
          cause instanceof Error
            ? cause.message
            : t(($) => $.create_project.builder_start_failed),
        );
        return null;
      })
      .finally(() => {
        createSessionRequestRef.current = null;
        setStarting(false);
      });
    createSessionRequestRef.current = request;
    return request;
  }, [agent, t]);

  const send = useCallback(
    async (
      content: string,
      attachmentIds?: string[],
      attachments: Attachment[] = [],
      commitInput?: () => void,
    ): Promise<boolean> => {
      const text = content.trim();
      if (!text || pending || !agent) return false;

      const targetSessionId = await ensureSession();
      // A close can happen while the lazy session request is in flight. The
      // cleanup below then removes the session and no message is sent into a
      // conversation the user can no longer see.
      if (!targetSessionId || !mountedRef.current) return false;

      setError(null);
      try {
        const encodedContent = encodeProjectBuilderInput(text, draft, catalogs);
        const result = await api.sendChatMessage(
          targetSessionId,
          encodedContent,
          attachmentIds,
        );

        // Prime both chat caches before publishing the session id. The first
        // render of the newly-created conversation therefore has the accepted
        // user message and uploaded attachments available immediately.
        upsertChatMessageToCaches(
          queryClient,
          targetSessionId,
          {
            id: result.message_id,
            chat_session_id: targetSessionId,
            role: "user",
            content: encodedContent,
            task_id: result.task_id,
            created_at: result.created_at,
            attachments,
          },
          { seedIfMissing: true },
        );
        queryClient.setQueryData(chatKeys.pendingTask(targetSessionId), {
          task_id: result.task_id,
          status: "queued",
          created_at: result.created_at,
        });
        setSessionId(targetSessionId);
        commitInput?.();
        void queryClient.invalidateQueries({
          queryKey: chatKeys.messages(targetSessionId),
        });
        void queryClient.invalidateQueries({
          queryKey: chatKeys.messagesPage(targetSessionId),
        });
        void queryClient.invalidateQueries({
          queryKey: chatKeys.pendingTask(targetSessionId),
        });
        return true;
      } catch (cause) {
        setError(
          cause instanceof Error
            ? cause.message
            : t(($) => $.create_project.builder_send_failed),
        );
        return false;
      }
    },
    [agent, catalogs, draft, ensureSession, pending, queryClient, t],
  );

  const stop = useCallback(async () => {
    const taskId = pendingTask?.task_id;
    if (!taskId || !sessionId) return;
    queryClient.setQueryData(chatKeys.pendingTask(sessionId), {});
    try {
      const result = await api.cancelTaskById(taskId);
      const restored = result.cancelled_chat_message;
      if (restored) {
        removeChatMessageFromCaches(
          queryClient,
          restored.chat_session_id,
          restored.message_id,
        );
        if (restored.restore_to_input) {
          setLocalRestore({
            id: restored.message_id,
            content: decodeProjectBuilderInput(restored.content),
            attachments: restored.attachments,
            sessionId,
          });
        }
      }
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: chatKeys.messages(sessionId) }),
        queryClient.invalidateQueries({ queryKey: chatKeys.messagesPage(sessionId) }),
        queryClient.invalidateQueries({ queryKey: chatKeys.pendingTask(sessionId) }),
      ]);
    } catch (cause) {
      setError(
        cause instanceof Error
          ? cause.message
          : t(($) => $.create_project.builder_stop_failed),
      );
      void queryClient.invalidateQueries({ queryKey: chatKeys.messages(sessionId) });
      void queryClient.invalidateQueries({ queryKey: chatKeys.messagesPage(sessionId) });
      void queryClient.invalidateQueries({ queryKey: chatKeys.pendingTask(sessionId) });
    }
  }, [pendingTask?.task_id, queryClient, sessionId, t]);

  const destroySession = useCallback(
    (id: string, reportError: boolean): Promise<boolean> => {
      if (!id) return Promise.resolve(true);
      const existing = destroyRequestRef.current;
      if (existing?.sessionId === id) return existing.promise;

      if (reportError && mountedRef.current) {
        setClosing(true);
        setError(null);
      }

      const request: {
        sessionId: string;
        promise: Promise<boolean>;
      } = {
        sessionId: id,
        promise: Promise.resolve(false),
      };
      destroyRequestRef.current = request;
      request.promise = (async () => {
        try {
          await api.deleteChatSession(id);
          queryClient.removeQueries({ queryKey: chatKeys.messages(id) });
          queryClient.removeQueries({ queryKey: chatKeys.messagesPage(id) });
          queryClient.removeQueries({ queryKey: chatKeys.pendingTask(id) });
          if (sessionIdRef.current === id) {
            sessionIdRef.current = "";
            if (mountedRef.current) setSessionId("");
          }
          return true;
        } catch (cause) {
          if (reportError && mountedRef.current) {
            setError(
              cause instanceof Error
                ? cause.message
                : t(($) => $.create_project.builder_close_failed),
            );
          }
          return false;
        } finally {
          if (reportError && mountedRef.current) setClosing(false);
          if (destroyRequestRef.current === request) {
            destroyRequestRef.current = null;
          }
        }
      })();

      return request.promise;
    },
    [queryClient, t],
  );

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      const id = sessionIdRef.current;
      if (!id) return;
      // Give StrictMode's effect replay a chance to mark the hook mounted
      // again; a real unmount still retires the ephemeral builder session.
      queueMicrotask(() => {
        if (mountedRef.current) return;
        void destroySession(id, false);
      });
    };
  }, [destroySession]);

  const handleRestoreDraftApplied = useCallback(() => {
    if (localRestore) {
      setLocalRestore(null);
      return;
    }
    durableRestore.handleRestoreDraftApplied();
  }, [durableRestore, localRestore]);

  return {
    sessionId,
    messages,
    messagesLoading: messagesQuery.isLoading,
    pendingTask,
    pending,
    missing,
    starting,
    closing,
    error,
    restoreDraftRequest,
    handleRestoreDraftApplied,
    send,
    stop,
  };
}
