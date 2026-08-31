"use client";

import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import {
  Check,
  Clipboard,
  CornerUpLeft,
  CircleHelp,
  Hash,
  LoaderCircle,
  MessageSquareQuote,
  Plus,
  Send,
  X,
} from "lucide-react";
import { toast } from "sonner";
import { Button } from "@patchbay/ui/components/ui/button";
import { Badge } from "@patchbay/ui/components/ui/badge";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@patchbay/ui/components/ui/dialog";
import { Input } from "@patchbay/ui/components/ui/input";
import { Textarea } from "@patchbay/ui/components/ui/textarea";
import { cn } from "@patchbay/ui/lib/utils";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { useWorkspacePaths } from "@patchbay/core/paths";
import { useCreateChannel, useSendChannelMessage } from "@patchbay/core/channels/mutations";
import { channelListOptions, channelMessagesOptions } from "@patchbay/core/channels/queries";
import { agentListOptions, memberListOptions } from "@patchbay/core/workspace/queries";
import type { Channel, ChannelMessage, ChannelQuotedMessage, MemberWithUser, Agent } from "@patchbay/core/types";
import { ContentEditor, ReadonlyContent, type ContentEditorRef } from "../editor";
import { ActorAvatar } from "../common/actor-avatar";
import { PageHeader } from "../layout/page-header";
import { useNavigation } from "../navigation";
import { useT, useTimeAgo } from "../i18n";

const EMPTY_CHANNELS: Channel[] = [];
const EMPTY_MEMBERS: MemberWithUser[] = [];
const EMPTY_AGENTS: Agent[] = [];

type MessageReference = {
  kind: "reply" | "quote";
  message: ChannelMessage;
};

function channelHref(base: string, channelId: string): string {
  return `${base}?channel=${encodeURIComponent(channelId)}`;
}

export function shouldSyncChannelUrl(
  pathname: string,
  channelsPath: string,
  urlChannelId: string | null,
  activeChannelId: string,
): boolean {
  return pathname === channelsPath && urlChannelId !== activeChannelId;
}

function ParticipantStack({ members, agents }: { members: MemberWithUser[]; agents: Agent[] }) {
  const participants = [
    ...members.map((member) => ({ type: "member", id: member.user_id })),
    ...agents
      .filter((agent) => !agent.archived_at)
      .map((agent) => ({ type: "agent", id: agent.id })),
  ];
  return (
    <div className="flex -space-x-1.5">
      {participants.slice(0, 5).map((participant) => (
        <ActorAvatar
          key={`${participant.type}:${participant.id}`}
          actorType={participant.type}
          actorId={participant.id}
          size="xs"
          profileLink={false}
          showStatusDot={participant.type === "agent"}
          className="ring-2 ring-background"
        />
      ))}
      {participants.length > 5 && (
        <span className="flex size-5 items-center justify-center rounded-full bg-muted text-micro font-medium text-muted-foreground ring-2 ring-background">
          +{participants.length - 5}
        </span>
      )}
    </div>
  );
}

function MessageReferencePreview({
  reference,
  onClear,
}: {
  reference: MessageReference;
  onClear: () => void;
}) {
  const { t } = useT("chat");
  const label = reference.kind === "reply"
    ? t(($) => $.channels.replying_to, { name: reference.message.author_name })
    : t(($) => $.channels.quoted_prefix, { name: reference.message.author_name });
  return (
    <div className="flex items-center gap-2 border-b bg-muted/30 px-3 py-2 text-caption">
      {reference.kind === "reply" ? (
        <CornerUpLeft className="size-3.5 text-muted-foreground" />
      ) : (
        <MessageSquareQuote className="size-3.5 text-muted-foreground" />
      )}
      <span className="min-w-0 flex-1 truncate text-muted-foreground">
        <span className="font-medium text-foreground">{label}</span>
        <span className="ml-2">{reference.message.content.replace(/\s+/g, " ")}</span>
      </span>
      <Button
        type="button"
        variant="ghost"
        size="icon-xs"
        onClick={onClear}
        aria-label={t(($) => $.channels.clear_reference)}
        title={t(($) => $.channels.clear_reference)}
      >
        <X />
      </Button>
    </div>
  );
}

function ChannelMessageRow({
  message,
  parent,
  onReply,
  onQuote,
  onCopy,
  timeAgo,
}: {
  message: ChannelMessage;
  parent: ChannelQuotedMessage | undefined;
  onReply: (message: ChannelMessage) => void;
  onQuote: (message: ChannelMessage) => void;
  onCopy: (message: ChannelMessage) => void;
  timeAgo: (date: string) => string;
}) {
  const { t } = useT("chat");
  const isAgent = message.author_type === "agent";
  const hasKnownActor = message.author_type === "member" || message.author_type === "agent";
  return (
    <article className="group flex gap-3 rounded-lg px-3 py-2.5 transition-colors hover:bg-muted/35 [content-visibility:auto]">
      {hasKnownActor ? (
        <ActorAvatar
          actorType={message.author_type}
          actorId={message.author_id}
          size="md"
          enableHoverCard
          showStatusDot={isAgent}
        />
      ) : (
        <span
          className="flex size-8 shrink-0 items-center justify-center rounded-full bg-muted text-muted-foreground"
          aria-label={t(($) => $.channels.unknown_badge)}
          title={t(($) => $.channels.unknown_badge)}
        >
          <CircleHelp className="size-4" aria-hidden="true" />
        </span>
      )}
      <div className="min-w-0 flex-1">
        <header className="flex flex-wrap items-center gap-2">
          <span className="text-body-sm font-semibold text-foreground">{message.author_name}</span>
          <Badge variant="outline" className={cn("h-4 px-1.5 text-micro", isAgent && "border-brand/30 bg-brand/10 text-brand")}>
            {isAgent
              ? t(($) => $.channels.agent_badge)
              : hasKnownActor
                ? t(($) => $.channels.member_badge)
                : t(($) => $.channels.unknown_badge)}
          </Badge>
          <time dateTime={message.created_at} className="text-caption text-muted-foreground">
            {timeAgo(message.created_at)}
          </time>
        </header>
        {parent && (
          <div className="mt-1 border-l-2 border-muted-foreground/25 pl-2 text-caption text-muted-foreground">
            <span className="font-medium">{parent.author_name}</span>
            <span className="ml-1">{parent.content.replace(/\s+/g, " ")}</span>
          </div>
        )}
        {message.quoted_message && (
          <div className="mt-2 rounded-md border-l-2 border-brand/60 bg-muted/35 px-2.5 py-1.5 text-caption">
            <div className="font-medium text-muted-foreground">
              {t(($) => $.channels.quoted_prefix, { name: message.quoted_message.author_name })}
            </div>
            <div className="mt-0.5 line-clamp-2 text-foreground">
              {message.quoted_message.content.replace(/\s+/g, " ")}
            </div>
          </div>
        )}
        <ReadonlyContent content={message.content} className="mt-1.5 leading-relaxed" />
        <div className="mt-1 flex items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
          <Button type="button" variant="ghost" size="xs" onClick={() => onReply(message)} title={t(($) => $.channels.reply)}>
            <CornerUpLeft />
            <span>{t(($) => $.channels.reply)}</span>
          </Button>
          <Button type="button" variant="ghost" size="xs" onClick={() => onQuote(message)} title={t(($) => $.channels.quote)}>
            <MessageSquareQuote />
            <span>{t(($) => $.channels.quote)}</span>
          </Button>
          <Button type="button" variant="ghost" size="xs" onClick={() => onCopy(message)} title={t(($) => $.channels.copy)}>
            <Clipboard />
            <span>{t(($) => $.channels.copy)}</span>
          </Button>
        </div>
      </div>
    </article>
  );
}

export function ChannelsPage() {
  const { t } = useT("chat");
  const { pathname, searchParams, replace } = useNavigation();
  const workspacePaths = useWorkspacePaths();
  const channelsPath = workspacePaths.channels();
  const workspaceId = useWorkspaceId();
  const timeAgo = useTimeAgo();
  const editorRef = useRef<ContentEditorRef>(null);
  const [draft, setDraft] = useState("");
  const [reference, setReference] = useState<MessageReference | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [channelName, setChannelName] = useState("");
  const [channelDescription, setChannelDescription] = useState("");
  const urlChannelId = searchParams.get("channel");

  const { data: channels = EMPTY_CHANNELS, isPending: channelsPending } = useQuery(
    channelListOptions(workspaceId),
  );
  const { data: members = EMPTY_MEMBERS } = useQuery(memberListOptions(workspaceId));
  const { data: agents = EMPTY_AGENTS } = useQuery(agentListOptions(workspaceId));
  const activeChannel = channels.find((channel) => channel.id === urlChannelId) ?? channels[0] ?? null;
  const {
    data: messagePages,
    isPending: messagesPending,
    fetchNextPage: fetchOlderMessages,
    hasNextPage: hasOlderMessages,
    isFetchingNextPage: isFetchingOlderMessages,
  } = useInfiniteQuery(
    channelMessagesOptions(activeChannel?.id ?? ""),
  );
  const createChannel = useCreateChannel();
  const sendMessage = useSendChannelMessage();
  const activeChannelIdRef = useRef<string | null>(activeChannel?.id ?? null);
  activeChannelIdRef.current = activeChannel?.id ?? null;
  const messageViewportRef = useRef<HTMLDivElement>(null);
  const messageBottomRef = useRef<HTMLDivElement>(null);
  const stickToBottomRef = useRef(true);
  const pendingPrependScrollRef = useRef<{ height: number; top: number } | null>(null);

  const messages = useMemo(() => {
    const seen = new Set<string>();
    return [...(messagePages?.pages ?? [])]
      .reverse()
      .flatMap((page) => page.messages)
      .filter((message) => {
        if (seen.has(message.id)) return false;
        seen.add(message.id);
        return true;
      });
  }, [messagePages]);

  useEffect(() => {
    if (!activeChannel) return;
    if (shouldSyncChannelUrl(pathname, channelsPath, urlChannelId, activeChannel.id)) {
      replace(channelHref(channelsPath, activeChannel.id));
    }
  }, [activeChannel, channelsPath, pathname, replace, urlChannelId]);

  useEffect(() => {
    setDraft("");
    setReference(null);
    stickToBottomRef.current = true;
  }, [activeChannel?.id]);

  useEffect(() => {
    if (messagesPending || pendingPrependScrollRef.current || !stickToBottomRef.current) return;
    messageBottomRef.current?.scrollIntoView({ block: "end" });
  }, [activeChannel?.id, messages.length, messagesPending]);

  const messageById = useMemo(
    () => new Map(messages.map((message) => [message.id, message])),
    [messages],
  );
  const participantCount = members.length + agents.filter((agent) => !agent.archived_at).length;
  const selectChannel = (channel: Channel) => {
    setReference(null);
    replace(channelHref(channelsPath, channel.id));
  };

  const clearComposer = () => {
    editorRef.current?.clearContent();
    setDraft("");
    setReference(null);
  };

  const handleMessageScroll = () => {
    const viewport = messageViewportRef.current;
    if (!viewport) return;
    stickToBottomRef.current =
      viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight <= 96;
  };

  const loadOlderMessages = async () => {
    const viewport = messageViewportRef.current;
    if (!viewport || !hasOlderMessages || isFetchingOlderMessages) return;
    stickToBottomRef.current = false;
    pendingPrependScrollRef.current = {
      height: viewport.scrollHeight,
      top: viewport.scrollTop,
    };
    await fetchOlderMessages();
    requestAnimationFrame(() => {
      const current = messageViewportRef.current;
      const previous = pendingPrependScrollRef.current;
      if (current && previous) {
        current.scrollTop = current.scrollHeight - previous.height + previous.top;
      }
      pendingPrependScrollRef.current = null;
    });
  };

  const handleSend = async () => {
    if (!activeChannel || sendMessage.isPending) return;
    const channelId = activeChannel.id;
    const content = editorRef.current?.getMarkdown().trim() || draft.trim();
    if (!content) return;
    try {
      await sendMessage.mutateAsync({
        channelId,
        content,
        parent_id: reference?.kind === "reply" ? reference.message.id : null,
        quoted_message_id: reference?.kind === "quote" ? reference.message.id : null,
      });
      if (activeChannelIdRef.current === channelId) clearComposer();
    } catch {
      toast.error(t(($) => $.channels.send_failed));
    }
  };

  const handleCreate = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const name = channelName.trim();
    if (!name || createChannel.isPending) return;
    try {
      const created = await createChannel.mutateAsync({
        name,
        description: channelDescription.trim(),
      });
      setChannelName("");
      setChannelDescription("");
      setCreateOpen(false);
      replace(channelHref(channelsPath, created.id));
    } catch {
      toast.error(t(($) => $.channels.create_failed));
    }
  };

  const copyMessage = async (message: ChannelMessage) => {
    try {
      if (!navigator.clipboard) throw new Error("clipboard unavailable");
      await navigator.clipboard.writeText(message.content);
      toast.success(t(($) => $.channels.copied));
    } catch {
      toast.error(t(($) => $.channels.copy_failed));
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      <PageHeader>
        <div className="flex min-w-0 flex-1 items-center gap-2">
          <Hash className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
          <h1 className="truncate font-heading text-title-sm font-semibold">{t(($) => $.channels.title)}</h1>
          <span className="hidden truncate text-caption text-muted-foreground sm:inline">{t(($) => $.channels.subtitle)}</span>
        </div>
        <Button
          type="button"
          size="sm"
          onClick={() => setCreateOpen(true)}
          aria-label={t(($) => $.channels.new_channel)}
          title={t(($) => $.channels.new_channel)}
        >
          <Plus />
          <span className="hidden sm:inline">{t(($) => $.channels.new_channel)}</span>
        </Button>
      </PageHeader>

      <div className="flex min-h-0 flex-1 overflow-hidden">
        <aside className="hidden w-60 shrink-0 flex-col border-r bg-muted/15 md:flex" aria-label={t(($) => $.channels.title)}>
          <div className="flex h-11 items-center justify-between border-b px-3">
            <span className="text-caption font-semibold uppercase tracking-wide text-muted-foreground">{t(($) => $.channels.title)}</span>
            <Button type="button" variant="ghost" size="icon-xs" onClick={() => setCreateOpen(true)} title={t(($) => $.channels.new_channel)} aria-label={t(($) => $.channels.new_channel)}>
              <Plus />
            </Button>
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto p-2">
            {channelsPending ? (
              <div className="space-y-2 p-2" role="status" aria-live="polite">
                <span className="sr-only">{t(($) => $.channels.loading)}</span>
                <div className="h-7 animate-pulse rounded-md bg-muted" />
                <div className="h-7 animate-pulse rounded-md bg-muted" />
              </div>
            ) : channels.length === 0 ? (
              <button type="button" className="w-full rounded-md p-2 text-left text-caption text-muted-foreground hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" onClick={() => setCreateOpen(true)}>
                {t(($) => $.channels.no_channels)}
              </button>
            ) : (
              <div className="space-y-0.5">
                {channels.map((channel) => (
                  <button
                    key={channel.id}
                    type="button"
                    onClick={() => selectChannel(channel)}
                    aria-pressed={activeChannel?.id === channel.id}
                    className={cn(
                      "flex w-full items-center gap-2 rounded-md px-2.5 py-2 text-left text-body-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                      activeChannel?.id === channel.id ? "bg-accent font-medium text-foreground" : "text-muted-foreground hover:bg-muted hover:text-foreground",
                    )}
                  >
                    <Hash className="size-4 shrink-0" aria-hidden="true" />
                    <span className="min-w-0 flex-1 truncate">{channel.name}</span>
                  </button>
                ))}
              </div>
            )}
          </div>
        </aside>

        <main className="flex min-w-0 flex-1 flex-col">
          {activeChannel ? (
            <>
              <header className="flex min-h-14 shrink-0 items-center gap-3 border-b px-4 py-2">
                <div className="flex min-w-0 flex-1 items-center gap-2">
                  <Hash className="size-5 shrink-0 text-muted-foreground" aria-hidden="true" />
                  <div className="min-w-0">
                    <h2 className="truncate text-body font-semibold">{activeChannel.name}</h2>
                    {activeChannel.description && <p className="truncate text-caption text-muted-foreground">{activeChannel.description}</p>}
                  </div>
                </div>
                <div className="hidden items-center gap-2 sm:flex">
                  <ParticipantStack members={members} agents={agents} />
                  <span className="text-caption text-muted-foreground">{t(($) => $.channels.participants, { count: participantCount })}</span>
                </div>
              </header>

              <nav className="flex gap-1 overflow-x-auto border-b px-2 py-2 md:hidden" aria-label={t(($) => $.channels.title)}>
                {channels.map((channel) => (
                  <button
                    key={channel.id}
                    type="button"
                    onClick={() => selectChannel(channel)}
                    aria-pressed={activeChannel.id === channel.id}
                    className={cn(
                      "flex shrink-0 items-center gap-1.5 rounded-md px-2.5 py-1.5 text-body-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                      activeChannel.id === channel.id ? "bg-accent font-medium text-foreground" : "text-muted-foreground hover:bg-muted hover:text-foreground",
                    )}
                  >
                    <Hash className="size-3.5" aria-hidden="true" />
                    <span>{channel.name}</span>
                  </button>
                ))}
              </nav>

              <div
                ref={messageViewportRef}
                onScroll={handleMessageScroll}
                className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-1 py-3 sm:px-3"
                aria-busy={messagesPending}
              >
                {messagesPending ? (
                  <div className="space-y-4 px-3" role="status" aria-live="polite">
                    <span className="sr-only">{t(($) => $.channels.loading)}</span>
                    <div className="h-12 animate-pulse rounded-lg bg-muted/50" />
                    <div className="h-16 animate-pulse rounded-lg bg-muted/50" />
                  </div>
                ) : messages.length === 0 ? (
                  <div className="flex h-full min-h-56 flex-col items-center justify-center gap-2 px-6 text-center">
                    <span className="flex size-10 items-center justify-center rounded-xl bg-brand/10 text-brand"><Hash className="size-5" aria-hidden="true" /></span>
                    <h3 className="text-body font-semibold">{t(($) => $.channels.empty_title)}</h3>
                    <p className="max-w-sm text-caption text-muted-foreground">{t(($) => $.channels.empty_message)}</p>
                  </div>
                ) : (
                  <div className="mx-auto max-w-4xl space-y-1">
                    {hasOlderMessages && (
                      <div className="flex justify-center pb-2">
                        <Button
                          type="button"
                          variant="ghost"
                          size="xs"
                          onClick={() => void loadOlderMessages()}
                          disabled={isFetchingOlderMessages}
                        >
                          {isFetchingOlderMessages && <LoaderCircle className="animate-spin" />}
                          {isFetchingOlderMessages
                            ? t(($) => $.channels.loading_older)
                            : t(($) => $.channels.load_older)}
                        </Button>
                      </div>
                    )}
                    {messages.map((message) => (
                      <ChannelMessageRow
                        key={message.id}
                        message={message}
                        parent={message.parent_message ?? (message.parent_id ? messageById.get(message.parent_id) : undefined)}
                        onReply={(value) => setReference({ kind: "reply", message: value })}
                        onQuote={(value) => setReference({ kind: "quote", message: value })}
                        onCopy={copyMessage}
                        timeAgo={timeAgo}
                      />
                    ))}
                    <div ref={messageBottomRef} aria-hidden="true" />
                  </div>
                )}
              </div>

              <div className="shrink-0 border-t bg-background p-3 sm:p-4">
                <div className="mx-auto max-w-4xl overflow-hidden rounded-xl border bg-muted/15 shadow-xs focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/20">
                  {reference && <MessageReferencePreview reference={reference} onClear={() => setReference(null)} />}
                  <div className="min-h-20 px-3 py-2">
                    <ContentEditor
                      key={activeChannel.id}
                      ref={editorRef}
                      defaultValue=""
                      placeholder={t(($) => $.channels.message_placeholder, { name: activeChannel.name })}
                      onUpdate={(markdown) => setDraft(markdown)}
                      onSubmit={() => void handleSend()}
                      mentionMode="default"
                      showBubbleMenu={false}
                      className="min-h-16"
                    />
                  </div>
                  <div className="flex items-center justify-between border-t px-2 py-2">
                    <span className="text-micro text-muted-foreground">{t(($) => $.channels.mention_hint)}</span>
                    <Button type="button" size="sm" disabled={sendMessage.isPending} onClick={() => void handleSend()}>
                      {sendMessage.isPending ? <LoaderCircle className="animate-spin" /> : <Send />}
                      <span>{t(($) => $.channels.send)}</span>
                    </Button>
                  </div>
                </div>
              </div>
            </>
          ) : (
            <div className="flex h-full flex-col items-center justify-center gap-3 px-6 text-center">
              <span className="flex size-12 items-center justify-center rounded-2xl bg-brand/10 text-brand"><Hash className="size-6" aria-hidden="true" /></span>
              <h2 className="text-title-sm font-semibold">{t(($) => $.channels.empty_title)}</h2>
              <p className="max-w-sm text-body-sm text-muted-foreground">{t(($) => $.channels.empty_description)}</p>
              <Button type="button" onClick={() => setCreateOpen(true)}><Plus />{t(($) => $.channels.new_channel)}</Button>
            </div>
          )}
        </main>
      </div>

      <Dialog open={createOpen} onOpenChange={setCreateOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t(($) => $.channels.create_title)}</DialogTitle>
            <DialogDescription>{t(($) => $.channels.create_description)}</DialogDescription>
          </DialogHeader>
          <form className="space-y-4" onSubmit={handleCreate}>
            <div className="space-y-1.5">
              <label htmlFor="channel-name" className="text-caption font-medium">{t(($) => $.channels.name_label)}</label>
              <Input id="channel-name" name="name" autoComplete="off" value={channelName} onChange={(event) => setChannelName(event.target.value)} placeholder={t(($) => $.channels.name_placeholder)} />
            </div>
            <div className="space-y-1.5">
              <label htmlFor="channel-description" className="text-caption font-medium">{t(($) => $.channels.description_label)}</label>
              <Textarea id="channel-description" name="description" autoComplete="off" value={channelDescription} onChange={(event) => setChannelDescription(event.target.value)} placeholder={t(($) => $.channels.description_placeholder)} rows={3} />
            </div>
            <DialogFooter>
              <Button type="button" variant="outline" onClick={() => setCreateOpen(false)}>{t(($) => $.channels.cancel)}</Button>
              <Button type="submit" disabled={createChannel.isPending || !channelName.trim()}>
                {createChannel.isPending ? <LoaderCircle className="animate-spin" /> : <Check />}
                {t(($) => $.channels.create)}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </div>
  );
}
