import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ActivityIndicator,
  FlatList,
  KeyboardAvoidingView,
  Platform,
  Pressable,
  RefreshControl,
  ScrollView,
  View,
} from "react-native";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { Stack, router, useLocalSearchParams } from "expo-router";
import type { WorkspaceChannel } from "@/data/channel-types";
import type { WorkspaceChannelMessageCacheEntry } from "@/data/channel-types";
import { channelListOptions, channelMessagesOptions } from "@/data/queries/channels";
import { useCreateWorkspaceChannelMessage } from "@/data/mutations/channels";
import { useAuthStore } from "@/data/auth-store";
import { useWorkspaceStore } from "@/data/workspace-store";
import { ActorAvatar } from "@/components/ui/actor-avatar";
import { AutosizeTextArea } from "@/components/ui/autosize-textarea";
import { Button } from "@/components/ui/button";
import { IconButton } from "@/components/ui/icon-button";
import { Text } from "@/components/ui/text";
import { useActorLookup } from "@/data/use-actor-name";
import {
  flattenChannelMessages,
} from "@/data/realtime/channel-ws-updaters";
import { formatChannelTimestamp } from "@/data/channel-types";
import { getW8Copy, normalizeW8Locale } from "@/lib/w8-copy";

export default function ChannelsPage() {
  const { channel: channelParam } = useLocalSearchParams<{
    channel?: string | string[];
  }>();
  const user = useAuthStore((state) => state.user);
  const wsId = useWorkspaceStore((state) => state.currentWorkspaceId);
  const wsSlug = useWorkspaceStore((state) => state.currentWorkspaceSlug);
  const copy = getW8Copy(user?.language);
  const { data, isLoading, error, refetch, isRefetching } = useQuery(
    channelListOptions(wsId),
  );
  const [selectedId, setSelectedId] = useState<string | null>(
    Array.isArray(channelParam) ? channelParam[0] ?? null : channelParam ?? null,
  );
  const activeChannels = useMemo(
    () => (data ?? []).filter((channel) => !channel.archived_at),
    [data],
  );
  const selectedChannel =
    activeChannels.find((channel) => channel.id === selectedId) ??
    activeChannels[0] ??
    null;

  useEffect(() => {
    if (selectedChannel && selectedId !== selectedChannel.id) {
      setSelectedId(selectedChannel.id);
    }
  }, [selectedChannel, selectedId]);

  const goCreate = useCallback(() => {
    if (wsSlug) router.push(`/${wsSlug}/channels/new`);
  }, [wsSlug]);

  const headerRight = useCallback(
    () => (
      <IconButton
        name="add"
        onPress={goCreate}
        accessibilityLabel={copy.channel.newChannel}
      />
    ),
    [copy.channel.newChannel, goCreate],
  );

  return (
    <>
      <Stack.Screen options={{ title: copy.channel.title, headerRight }} />
      <KeyboardAvoidingView
        className="flex-1 bg-background"
        behavior={Platform.OS === "ios" ? "padding" : undefined}
      >
        <View className="border-b border-border">
          <ScrollView
            horizontal
            contentContainerClassName="px-4 py-3 gap-2"
            showsHorizontalScrollIndicator={false}
          >
            {activeChannels.map((channel) => (
              <ChannelChip
                key={channel.id}
                channel={channel}
                selected={channel.id === selectedChannel?.id}
                onPress={() => setSelectedId(channel.id)}
              />
            ))}
          </ScrollView>
        </View>

        {isLoading ? (
          <View className="flex-1 items-center justify-center">
            <ActivityIndicator />
          </View>
        ) : error ? (
          <View className="flex-1 items-center justify-center px-6 gap-3">
            <Text className="text-sm text-destructive text-center">
              {copy.channel.loadFailed}
            </Text>
            <Button variant="outline" onPress={() => refetch()}>
              <Text>{copy.channel.retry}</Text>
            </Button>
          </View>
        ) : activeChannels.length === 0 ? (
          <EmptyChannels onCreate={goCreate} copy={copy.channel} />
        ) : selectedChannel && wsId ? (
          <ChannelConversation
            key={selectedChannel.id}
            channel={selectedChannel}
            wsId={wsId}
            locale={normalizeW8Locale(user?.language)}
            copy={copy.channel}
            isRefreshing={isRefetching}
            onRefresh={refetch}
          />
        ) : (
          <View className="flex-1 items-center justify-center px-6">
            <Text className="text-sm text-muted-foreground text-center">
              {copy.channel.selectPrompt}
            </Text>
          </View>
        )}
      </KeyboardAvoidingView>
    </>
  );
}

function ChannelChip({
  channel,
  selected,
  onPress,
}: {
  channel: WorkspaceChannel;
  selected: boolean;
  onPress: () => void;
}) {
  return (
    <Pressable
      onPress={onPress}
      accessibilityRole="button"
      accessibilityState={{ selected }}
      className={selected ? "rounded-full bg-primary px-4 py-2" : "rounded-full bg-secondary px-4 py-2"}
    >
      <Text
        className={
          selected
            ? "text-sm font-medium text-primary-foreground"
            : "text-sm font-medium text-secondary-foreground"
        }
        numberOfLines={1}
      >
        {channel.name || channel.slug}
      </Text>
    </Pressable>
  );
}

function EmptyChannels({
  onCreate,
  copy,
}: {
  onCreate: () => void;
  copy: ReturnType<typeof getW8Copy>["channel"];
}) {
  return (
    <View className="flex-1 items-center justify-center px-6 gap-4">
      <Text className="text-base font-medium text-foreground">
        {copy.emptyTitle}
      </Text>
      <Text className="text-sm text-muted-foreground text-center">
        {copy.emptyDescription}
      </Text>
      <Button onPress={onCreate}>
        <Text>{copy.newChannel}</Text>
      </Button>
    </View>
  );
}

function ChannelConversation({
  channel,
  wsId,
  locale,
  copy,
  isRefreshing,
  onRefresh,
}: {
  channel: WorkspaceChannel;
  wsId: string;
  locale: string;
  copy: ReturnType<typeof getW8Copy>["channel"];
  isRefreshing: boolean;
  onRefresh: () => void;
}) {
  const listRef = useRef<FlatList<WorkspaceChannelMessageCacheEntry>>(null);
  const didInitialScroll = useRef(false);
  const { data, error, isLoading, isFetchingNextPage, hasNextPage, fetchNextPage } =
    useInfiniteQuery(channelMessagesOptions(wsId, channel.id));
  const messages = flattenChannelMessages(data?.pages);

  useEffect(() => {
    didInitialScroll.current = false;
  }, [channel.id]);

  const scrollToEnd = useCallback(() => {
    if (didInitialScroll.current || messages.length === 0) return;
    didInitialScroll.current = true;
    listRef.current?.scrollToEnd({ animated: false });
  }, [messages.length]);

  return (
    <View className="flex-1">
      <View className="px-4 py-3 gap-1">
        <Text className="text-lg font-semibold text-foreground">
          {channel.name || channel.slug}
        </Text>
        {channel.description ? (
          <Text className="text-sm text-muted-foreground">
            {channel.description}
          </Text>
        ) : null}
      </View>

      {isLoading ? (
        <View className="flex-1 items-center justify-center">
          <ActivityIndicator />
        </View>
      ) : error ? (
        <View className="flex-1 items-center justify-center px-6">
          <Text className="text-sm text-destructive text-center">
            {copy.messageFailed}
          </Text>
        </View>
      ) : (
        <FlatList
          ref={listRef}
          data={messages}
          keyExtractor={(message) => message.id}
          renderItem={({ item }) => <MessageRow message={item} locale={locale} />}
          contentContainerClassName="px-4 gap-3 pt-2 pb-3"
          ListHeaderComponent={
            hasNextPage ? (
              <View className="items-center pb-2">
                <Button
                  variant="outline"
                  size="sm"
                  onPress={() => void fetchNextPage()}
                  disabled={isFetchingNextPage}
                >
                  <Text>
                    {isFetchingNextPage
                      ? copy.loadingEarlier
                      : copy.loadEarlier}
                  </Text>
                </Button>
              </View>
            ) : null
          }
          ListEmptyComponent={
            <View className="items-center py-10">
              <Text className="text-sm text-muted-foreground">
                {copy.messagesEmpty}
              </Text>
            </View>
          }
          refreshControl={
            <RefreshControl refreshing={isRefreshing} onRefresh={onRefresh} />
          }
          onContentSizeChange={scrollToEnd}
        />
      )}

      <ChannelComposer channelId={channel.id} copy={copy} />
    </View>
  );
}

function MessageRow({
  message,
  locale,
}: {
  message: WorkspaceChannelMessageCacheEntry;
  locale: string;
}) {
  const { getName } = useActorLookup();
  const actorType =
    message.author_type === "member" ||
    message.author_type === "agent" ||
    message.author_type === "team"
      ? message.author_type
      : null;
  const authorName = actorType
    ? getName(actorType, message.author_id)
    : "System";
  return (
    <View className="flex-row gap-3">
      <ActorAvatar type={actorType} id={message.author_id} size={32} />
      <View className="flex-1 gap-1">
        <View className="flex-row items-baseline gap-2">
          <Text className="text-sm font-semibold text-foreground">
            {authorName}
          </Text>
          <Text className="text-xs text-muted-foreground">
            {formatChannelTimestamp(message.created_at, locale)}
          </Text>
        </View>
        <Text className="text-sm text-foreground">{message.content}</Text>
      </View>
    </View>
  );
}

function ChannelComposer({
  channelId,
  copy,
}: {
  channelId: string;
  copy: ReturnType<typeof getW8Copy>["channel"];
}) {
  const [draft, setDraft] = useState("");
  const [error, setError] = useState(false);
  const createMessage = useCreateWorkspaceChannelMessage();
  const canSend = draft.trim().length > 0 && !createMessage.isPending;

  const send = useCallback(() => {
    const content = draft.trim();
    if (!content || createMessage.isPending) return;
    setError(false);
    setDraft("");
    createMessage.mutate(
      { channelId, content },
      {
        onError: () => {
          setDraft(content);
          setError(true);
        },
      },
    );
  }, [channelId, createMessage, draft]);

  return (
    <View className="border-t border-border bg-background px-3 py-2 gap-1">
      {error ? (
        <Text className="text-xs text-destructive px-1">{copy.messageFailed}</Text>
      ) : null}
      <View className="flex-row items-end gap-2">
        <AutosizeTextArea
          value={draft}
          onChangeText={(value) => {
            setDraft(value);
            if (error) setError(false);
          }}
          placeholder={copy.messagePlaceholder}
          className="flex-1 rounded-md border border-border bg-secondary/50 px-3 py-2"
          minHeight={40}
          maxHeight={120}
          editable={!createMessage.isPending}
          returnKeyType="send"
          blurOnSubmit={false}
          onSubmitEditing={send}
        />
        <IconButton
          name="send"
          onPress={send}
          disabled={!canSend}
          accessibilityLabel={copy.send}
        />
      </View>
    </View>
  );
}
