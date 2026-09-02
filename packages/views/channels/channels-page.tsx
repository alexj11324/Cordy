"use client";

import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { Hash, Loader2, Plus, RefreshCw, Send } from "lucide-react";
import type {
  WorkspaceChannel,
  WorkspaceChannelMessage,
} from "@patchbay/core/types";
import { useWorkspaceId } from "@patchbay/core";
import { useAuthStore } from "@patchbay/core/auth";
import { errorCode } from "@patchbay/core/api";
import {
  channelListOptions,
  channelMessagesOptions,
} from "@patchbay/core/channels/queries";
import {
  useCreateWorkspaceChannel,
  useCreateWorkspaceChannelMessage,
} from "@patchbay/core/channels/mutations";
import { useChannelRealtime } from "@patchbay/core/channels";
import { useActorName } from "@patchbay/core/workspace/hooks";
import { useWorkspacePaths } from "@patchbay/core/paths";
import { Button } from "@patchbay/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@patchbay/ui/components/ui/dialog";
import { Input } from "@patchbay/ui/components/ui/input";
import { Label } from "@patchbay/ui/components/ui/label";
import { Textarea } from "@patchbay/ui/components/ui/textarea";
import { ActorAvatar } from "@patchbay/ui/components/common/actor-avatar";
import {
  CollectionPageHeader,
  CollectionPageHeaderAction,
  CollectionPageState,
} from "../layout/collection-page";
import { useLocale, useT } from "../i18n";
import { useNavigation } from "../navigation";

/** Convert a human channel name into the stable slug expected by the Go API. */
export function channelSlugFromName(name: string): string {
  return name
    .trim()
    .toLocaleLowerCase()
    .replace(/\s+/g, "-")
    .replace(/[^\p{L}\p{N}-]/gu, "")
    .replace(/-+/g, "-")
    .replace(/^-|-$/g, "");
}

export function formatChannelDate(value: string, locale: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "";
  return new Intl.DateTimeFormat(locale, {
    dateStyle: "short",
    timeStyle: "short",
  }).format(date);
}

/** The Go endpoint returns each cursor page chronologically, newest window first. */
export function flattenChannelMessagePages(
  pages: ReadonlyArray<{ messages: readonly WorkspaceChannelMessage[] }>,
): WorkspaceChannelMessage[] {
  return [...pages].reverse().flatMap((page) => page.messages);
}

export function ChannelsPage() {
  const { t } = useT("channels");
  const { searchParams, replace } = useNavigation();
  const workspaceId = useWorkspaceId();
  const workspacePaths = useWorkspacePaths();
  const channelsPath = workspacePaths.channels();
  useChannelRealtime(workspaceId);
  const channelsQuery = useQuery(channelListOptions(workspaceId));
  const createChannel = useCreateWorkspaceChannel();
  const [createOpen, setCreateOpen] = useState(false);
  const urlChannelId = searchParams.get("channel") ?? "";
  const [selectedId, setSelectedId] = useState(urlChannelId);

  const channels = useMemo(
    () => (channelsQuery.data?.channels ?? []).filter((channel) => !channel.archived_at),
    [channelsQuery.data?.channels],
  );

  // Keep the selected channel addressable and recover gracefully when a
  // deep-linked or archived channel is no longer present in the list.
  useEffect(() => {
    if (channelsQuery.isPending) return;
    const requested = channels.some((channel) => channel.id === urlChannelId)
      ? urlChannelId
      : "";
    const nextId = requested || channels[0]?.id || "";
    if (nextId === selectedId) return;
    setSelectedId(nextId);
    replace(nextId ? `${channelsPath}?channel=${encodeURIComponent(nextId)}` : channelsPath);
  }, [channels, channelsPath, channelsQuery.isPending, replace, selectedId, urlChannelId]);

  const selectChannel = (id: string) => {
    setSelectedId(id);
    replace(`${channelsPath}?channel=${encodeURIComponent(id)}`);
  };

  const selectedChannel = channels.find((channel) => channel.id === selectedId) ?? null;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <CollectionPageHeader
        icon={Hash}
        title={t(($) => $.page.title)}
        count={channels.length}
        description={t(($) => $.page.description)}
        actions={
          <CollectionPageHeaderAction
            icon={Plus}
            label={t(($) => $.page.new_button)}
            onClick={() => setCreateOpen(true)}
          />
        }
      />

      <div className="flex min-h-0 flex-1 flex-col md:flex-row">
        <aside
          aria-label={t(($) => $.page.title)}
          className="w-full shrink-0 border-b md:w-72 md:border-r md:border-b-0"
        >
          {channelsQuery.isPending ? (
            <ChannelListSkeleton />
          ) : channelsQuery.isError ? (
            <CollectionPageState
              icon={Hash}
              title={t(($) => $.page.error_title)}
              description={t(($) => $.page.error_description)}
              role="alert"
              className="py-10"
              actions={
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => void channelsQuery.refetch()}
                >
                  <RefreshCw aria-hidden="true" />
                  {t(($) => $.page.retry)}
                </Button>
              }
            />
          ) : channels.length === 0 ? (
            <CollectionPageState
              icon={Hash}
              title={t(($) => $.page.empty_title)}
              description={t(($) => $.page.empty_description)}
              className="py-10"
              actions={
                <Button size="sm" onClick={() => setCreateOpen(true)}>
                  <Plus aria-hidden="true" />
                  {t(($) => $.page.new_button)}
                </Button>
              }
            />
          ) : (
            <nav className="flex max-h-[calc(100vh-7rem)] gap-1 overflow-x-auto p-2 md:block md:overflow-y-auto">
              {channels.map((channel) => (
                <ChannelListItem
                  key={channel.id}
                  channel={channel}
                  selected={channel.id === selectedId}
                  onSelect={() => selectChannel(channel.id)}
                />
              ))}
            </nav>
          )}
        </aside>

        <main className="flex min-h-0 min-w-0 flex-1">
          {selectedChannel ? (
            <ChannelConversation
              channel={selectedChannel}
              workspaceId={workspaceId}
            />
          ) : (
            <CollectionPageState
              icon={Hash}
              title={t(($) => $.page.select_prompt)}
              className="flex-1"
            />
          )}
        </main>
      </div>

      <CreateChannelDialog
        open={createOpen}
        onOpenChange={setCreateOpen}
        mutation={createChannel}
        onCreated={(channel) => selectChannel(channel.id)}
      />
    </div>
  );
}

function ChannelListSkeleton() {
  return (
    <div className="space-y-2 p-3" aria-hidden="true">
      {["one", "two", "three"].map((key) => (
        <div key={key} className="h-12 animate-pulse rounded-lg bg-muted/50" />
      ))}
    </div>
  );
}

function ChannelListItem({
  channel,
  selected,
  onSelect,
}: {
  channel: WorkspaceChannel;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      aria-current={selected ? "page" : undefined}
      onClick={onSelect}
      className={`flex min-w-56 flex-1 items-center gap-2 rounded-lg px-3 py-2 text-left transition-colors md:min-w-0 md:w-full ${
        selected
          ? "bg-sidebar-accent text-sidebar-accent-foreground"
          : "text-muted-foreground hover:bg-sidebar-accent/70 hover:text-foreground"
      }`}
    >
      <Hash aria-hidden="true" className="size-4 shrink-0" />
      <span className="min-w-0 flex-1">
        <span className="block truncate text-body font-medium">{channel.name}</span>
        <span className="block truncate text-caption text-muted-foreground">{channel.slug}</span>
      </span>
    </button>
  );
}

function CreateChannelDialog({
  open,
  onOpenChange,
  mutation,
  onCreated,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  mutation: ReturnType<typeof useCreateWorkspaceChannel>;
  onCreated: (channel: WorkspaceChannel) => void;
}) {
  const { t } = useT("channels");
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [description, setDescription] = useState("");
  const resetMutation = mutation.reset;

  useEffect(() => {
    if (!open) return;
    setName("");
    setSlug("");
    setDescription("");
    resetMutation();
  }, [open, resetMutation]);

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const cleanName = name.trim();
    const cleanSlug = (slug.trim() || channelSlugFromName(cleanName)).toLocaleLowerCase();
    if (!cleanName || !cleanSlug) {
      toast.error(t(($) => $.toasts.required));
      return;
    }

    try {
      const channel = await mutation.mutateAsync({
        name: cleanName,
        slug: cleanSlug,
        ...(description.trim() ? { description: description.trim() } : {}),
      });
      toast.success(t(($) => $.toasts.created));
      onOpenChange(false);
      onCreated(channel);
    } catch (error) {
      toast.error(
        errorCode(error) === "channel_conflict"
          ? t(($) => $.toasts.conflict)
          : t(($) => $.toasts.create_failed),
      );
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t(($) => $.channel.create_title)}</DialogTitle>
          <DialogDescription>{t(($) => $.channel.create_description)}</DialogDescription>
        </DialogHeader>
        <form className="space-y-4" onSubmit={(event) => void submit(event)}>
          <div className="space-y-2">
            <Label htmlFor="channel-name">{t(($) => $.channel.name)}</Label>
            <Input
              id="channel-name"
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder={t(($) => $.channel.name_placeholder)}
              autoFocus
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="channel-slug">{t(($) => $.channel.slug)}</Label>
            <Input
              id="channel-slug"
              value={slug}
              onChange={(event) => setSlug(event.target.value)}
              placeholder={t(($) => $.channel.slug_placeholder)}
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="channel-description">{t(($) => $.channel.description)}</Label>
            <Textarea
              id="channel-description"
              value={description}
              onChange={(event) => setDescription(event.target.value)}
              placeholder={t(($) => $.channel.description_placeholder)}
              rows={3}
            />
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => onOpenChange(false)}
            >
              {t(($) => $.channel.cancel)}
            </Button>
            <Button type="submit" disabled={mutation.isPending}>
              {mutation.isPending ? <Loader2 aria-hidden="true" className="animate-spin" /> : null}
              {mutation.isPending
                ? t(($) => $.channel.creating)
                : t(($) => $.channel.create)}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function ChannelConversation({
  channel,
  workspaceId,
}: {
  channel: WorkspaceChannel;
  workspaceId: string;
}) {
  const { t } = useT("channels");
  const locale = useLocale();
  const currentUserId = useAuthStore((state) => state.user?.id ?? "");
  const messagesQuery = useInfiniteQuery(channelMessagesOptions(workspaceId, channel.id));
  const sendMessage = useCreateWorkspaceChannelMessage();
  const { getActorName, getActorInitials, getActorAvatarUrl } = useActorName();
  const [draft, setDraft] = useState("");
  const transcriptRef = useRef<HTMLDivElement>(null);

  const loadEarlier = async () => {
    const transcript = transcriptRef.current;
    const previousHeight = transcript?.scrollHeight ?? 0;
    const previousTop = transcript?.scrollTop ?? 0;
    try {
      await messagesQuery.fetchNextPage();
    } finally {
      if (transcript && previousHeight > 0) {
        requestAnimationFrame(() => {
          transcript.scrollTop =
            previousTop + (transcript.scrollHeight - previousHeight);
        });
      }
    }
  };

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const content = draft.trim();
    if (!content || !currentUserId) return;

    setDraft("");
    try {
      await sendMessage.mutateAsync({
        channelId: channel.id,
        author_type: "member",
        author_id: currentUserId,
        content,
      });
    } catch {
      // The mutation removes its optimistic row on failure. Restore the text
      // only when the user has not started a new draft while the request was
      // in flight.
      setDraft((current) => current || content);
      toast.error(t(($) => $.toasts.message_failed));
    }
  };

  const messages = useMemo(
    () => flattenChannelMessagePages(messagesQuery.data?.pages ?? []),
    [messagesQuery.data?.pages],
  );

  return (
    <section className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header className="flex shrink-0 items-center justify-between gap-3 border-b px-4 py-3">
        <div className="min-w-0">
          <h2 className="truncate text-body font-semibold">{channel.name}</h2>
          {channel.description ? (
            <p className="truncate text-caption text-muted-foreground">{channel.description}</p>
          ) : null}
        </div>
        <span className="shrink-0 font-mono text-caption text-muted-foreground">#{channel.slug}</span>
      </header>

      <div
        ref={transcriptRef}
        role="log"
        aria-live="polite"
        className="min-h-0 flex-1 overflow-y-auto px-4 py-5"
      >
        {messagesQuery.isPending ? (
          <div className="flex items-center justify-center py-12 text-muted-foreground">
            <Loader2 aria-hidden="true" className="size-5 animate-spin" />
          </div>
        ) : messagesQuery.isError && !messagesQuery.data ? (
          <CollectionPageState
            icon={Hash}
            title={t(($) => $.page.error_title)}
            description={t(($) => $.page.error_description)}
            role="alert"
            actions={
              <Button
                variant="outline"
                size="sm"
                onClick={() => void messagesQuery.refetch()}
              >
                <RefreshCw aria-hidden="true" />
                {t(($) => $.page.retry)}
              </Button>
            }
          />
        ) : (
          <>
            {messagesQuery.hasNextPage ? (
              <div className="flex justify-center pb-4">
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={() => void loadEarlier()}
                  disabled={messagesQuery.isFetchingNextPage}
                >
                  {messagesQuery.isFetchingNextPage ? (
                    <Loader2 aria-hidden="true" className="animate-spin" />
                  ) : null}
                  {messagesQuery.isFetchingNextPage
                    ? t(($) => $.channel.loading_earlier)
                    : t(($) => $.channel.load_earlier)}
                </Button>
              </div>
            ) : null}
            {messages.length === 0 ? (
              <CollectionPageState
                icon={Hash}
                title={t(($) => $.channel.messages_empty)}
                className="py-12"
              />
            ) : (
              <div className="mx-auto flex max-w-3xl flex-col gap-5">
                {messages.map((message) => (
                  <ChannelMessageRow
                    key={message.id}
                    message={message}
                    locale={locale}
                    getActorName={getActorName}
                    getActorInitials={getActorInitials}
                    getActorAvatarUrl={getActorAvatarUrl}
                  />
                ))}
              </div>
            )}
          </>
        )}
      </div>

      <form
        className="flex shrink-0 items-end gap-2 border-t bg-surface/60 p-3"
        onSubmit={(event) => void submit(event)}
      >
        <Textarea
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          placeholder={t(($) => $.channel.message_placeholder)}
          aria-label={t(($) => $.channel.message_placeholder)}
          rows={2}
          className="min-h-10 resize-none"
          disabled={sendMessage.isPending}
        />
        <Button
          type="submit"
          size="icon"
          aria-label={t(($) => $.channel.send)}
          disabled={!draft.trim() || !currentUserId || sendMessage.isPending}
        >
          {sendMessage.isPending ? (
            <Loader2 aria-hidden="true" className="animate-spin" />
          ) : (
            <Send aria-hidden="true" />
          )}
        </Button>
      </form>
    </section>
  );
}

function ChannelMessageRow({
  message,
  locale,
  getActorName,
  getActorInitials,
  getActorAvatarUrl,
}: {
  message: WorkspaceChannelMessage;
  locale: string;
  getActorName: (type: string, id: string) => string;
  getActorInitials: (type: string, id: string) => string;
  getActorAvatarUrl: (type: string, id: string) => string | null;
}) {
  const name = getActorName(message.author_type, message.author_id);
  const time = formatChannelDate(message.created_at, locale);

  return (
    <article className="flex gap-3">
      <ActorAvatar
        name={name}
        initials={getActorInitials(message.author_type, message.author_id)}
        avatarUrl={getActorAvatarUrl(message.author_type, message.author_id)}
        isAgent={message.author_type === "agent"}
        isSystem={message.author_type === "system"}
        isTeam={message.author_type === "team"}
        size="sm"
      />
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5">
          <span className="text-body font-medium">{name}</span>
          {time ? (
            <time dateTime={message.created_at} className="text-caption text-muted-foreground">
              {time}
            </time>
          ) : null}
        </div>
		<p className="whitespace-pre-wrap break-words text-body text-foreground">{message.content}</p>
      </div>
    </article>
  );
}
