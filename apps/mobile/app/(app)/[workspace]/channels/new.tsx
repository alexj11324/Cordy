import { useCallback, useState } from "react";
import {
  KeyboardAvoidingView,
  Platform,
  Pressable,
  ScrollView,
  View,
} from "react-native";
import { Stack, router } from "expo-router";
import { AutosizeTextArea } from "@/components/ui/autosize-textarea";
import { Text } from "@/components/ui/text";
import { TextField } from "@/components/ui/text-field";
import { useCreateWorkspaceChannel } from "@/data/mutations/channels";
import { useAuthStore } from "@/data/auth-store";
import { channelSlugFromName } from "@/data/channel-types";
import { getW8Copy } from "@/lib/w8-copy";

export default function NewChannelPage() {
  const user = useAuthStore((state) => state.user);
  const copy = getW8Copy(user?.language);
  const createChannel = useCreateWorkspaceChannel();
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [slugTouched, setSlugTouched] = useState(false);
  const [description, setDescription] = useState("");
  const [error, setError] = useState<string | null>(null);

  const onNameChange = useCallback(
    (value: string) => {
      setName(value);
      if (!slugTouched) setSlug(channelSlugFromName(value));
    },
    [slugTouched],
  );

  const onCreate = useCallback(() => {
    const normalizedName = name.trim();
    const normalizedSlug = channelSlugFromName(slug);
    if (!normalizedName || !normalizedSlug) {
      setError(copy.channel.required);
      return;
    }
    setError(null);
    createChannel.mutate(
      {
        name: normalizedName,
        slug: normalizedSlug,
        description: description.trim() || undefined,
      },
      {
        onSuccess: () => router.back(),
        onError: (err) =>
          setError(
            err instanceof Error
              ? `${copy.channel.createFailed} ${err.message}`
              : copy.channel.createFailed,
          ),
      },
    );
  }, [copy.channel.createFailed, copy.channel.required, createChannel, description, name, slug]);

  return (
    <>
      <Stack.Screen options={{ headerShown: false }} />
      <KeyboardAvoidingView
        className="flex-1 bg-background"
        behavior={Platform.OS === "ios" ? "padding" : undefined}
      >
        <View className="flex-row items-center justify-between px-4 py-3 border-b border-border">
          <Pressable onPress={() => router.back()} className="px-1 py-1">
            <Text className="text-base text-brand">{copy.channel.cancel}</Text>
          </Pressable>
          <Text className="text-base font-semibold text-foreground">
            {copy.channel.createTitle}
          </Text>
          <Pressable
            onPress={onCreate}
            disabled={createChannel.isPending}
            className={createChannel.isPending ? "px-1 py-1 opacity-40" : "px-1 py-1"}
          >
            <Text className="text-base font-semibold text-brand">
              {createChannel.isPending
                ? copy.channel.creating
                : copy.channel.create}
            </Text>
          </Pressable>
        </View>

        <ScrollView
          className="flex-1"
          contentContainerClassName="px-4 pt-5 pb-8 gap-4"
          keyboardShouldPersistTaps="handled"
        >
          <View className="gap-1">
            <Text className="text-xl font-semibold text-foreground">
              {copy.channel.createTitle}
            </Text>
            <Text className="text-sm text-muted-foreground">
              {copy.channel.createDescription}
            </Text>
          </View>

          <Field label={copy.channel.name}>
            <TextField
              value={name}
              onChangeText={onNameChange}
              placeholder={copy.channel.namePlaceholder}
              autoFocus
              returnKeyType="next"
            />
          </Field>

          <Field label={copy.channel.slug}>
            <TextField
              value={slug}
              onChangeText={(value) => {
                setSlugTouched(true);
                setSlug(channelSlugFromName(value));
              }}
              placeholder={copy.channel.slugPlaceholder}
              autoCapitalize="none"
              autoCorrect={false}
            />
          </Field>

          <Field label={copy.channel.description}>
            <AutosizeTextArea
              value={description}
              onChangeText={setDescription}
              placeholder={copy.channel.descriptionPlaceholder}
              className="rounded-md border border-border bg-secondary/50 px-3 py-2"
              minHeight={96}
            />
          </Field>

          {error ? (
            <Text className="text-sm text-destructive">{error}</Text>
          ) : null}
        </ScrollView>
      </KeyboardAvoidingView>
    </>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <View className="gap-2">
      <Text className="text-sm font-medium text-foreground">{label}</Text>
      {children}
    </View>
  );
}
