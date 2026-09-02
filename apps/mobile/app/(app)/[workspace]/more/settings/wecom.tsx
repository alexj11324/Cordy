import { useCallback, useState } from "react";
import {
  ActionSheetIOS,
  ActivityIndicator,
  Alert,
  KeyboardAvoidingView,
  Platform,
  Pressable,
  ScrollView,
  View,
} from "react-native";
import { Stack } from "expo-router";
import { useQuery } from "@tanstack/react-query";
import { Text } from "@/components/ui/text";
import { TextField } from "@/components/ui/text-field";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { ApiError } from "@/data/api";
import { useAuthStore } from "@/data/auth-store";
import { useWorkspaceStore } from "@/data/workspace-store";
import { agentListOptions } from "@/data/queries/agents";
import { memberListOptions } from "@/data/queries/members";
import { wecomInstallationsOptions } from "@/data/queries/wecom";
import {
  useDisconnectWecomInstallation,
  useRegisterWecomBYO,
} from "@/data/mutations/wecom";
import { getW8Copy } from "@/lib/w8-copy";

export default function WecomSettingsPage() {
  const user = useAuthStore((state) => state.user);
  const wsId = useWorkspaceStore((state) => state.currentWorkspaceId);
  const copy = getW8Copy(user?.language);
  const installationsQuery = useQuery(wecomInstallationsOptions(wsId));
  const membersQuery = useQuery(memberListOptions(wsId));
  const agentsQuery = useQuery(agentListOptions(wsId));
  const register = useRegisterWecomBYO();
  const disconnect = useDisconnectWecomInstallation();
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);
  const [botId, setBotId] = useState("");
  const [secret, setSecret] = useState("");
  const [botName, setBotName] = useState("");
  const [formError, setFormError] = useState<string | null>(null);

  const currentMember = membersQuery.data?.find(
    (member) => member.user_id === user?.id,
  );
  // Fail closed while membership is loading or missing. The server remains
  // the final authorization boundary for both write endpoints.
  const canManage =
    currentMember?.role === "owner" || currentMember?.role === "admin";
  const agents = agentsQuery.data ?? [];
  const selectedAgent = agents.find((agent) => agent.id === selectedAgentId);
  const installations = installationsQuery.data?.installations ?? [];

  const chooseAgent = useCallback(() => {
    if (agents.length === 0) {
      Alert.alert(copy.wecom.selectAgent, copy.wecom.noAgents);
      return;
    }
    const options = [...agents.map((agent) => agent.name), copy.wecom.cancel];
    const cancelButtonIndex = options.length - 1;
    const select = (index: number) => {
      if (index >= 0 && index < cancelButtonIndex) {
        setSelectedAgentId(agents[index].id);
      }
    };
    if (Platform.OS === "ios") {
      ActionSheetIOS.showActionSheetWithOptions(
        {
          options,
          cancelButtonIndex,
          title: copy.wecom.selectAgent,
        },
        select,
      );
      return;
    }
    Alert.alert(
      copy.wecom.selectAgent,
      undefined,
      agents.map((agent) => ({
        text: agent.name,
        onPress: () => setSelectedAgentId(agent.id),
      })),
    );
  }, [agents, copy.wecom.cancel, copy.wecom.noAgents, copy.wecom.selectAgent]);

  const connect = useCallback(() => {
    if (!canManage) return;
    const normalizedBotId = botId.trim();
    const normalizedSecret = secret.trim();
    if (!selectedAgentId || !normalizedBotId || !normalizedSecret) {
      setFormError(copy.wecom.required);
      return;
    }
    setFormError(null);
    register.mutate(
      {
        agentId: selectedAgentId,
        body: {
          bot_id: normalizedBotId,
          secret: normalizedSecret,
          ...(botName.trim() ? { bot_name: botName.trim() } : {}),
        },
      },
      {
        onSuccess: () => {
          setBotId("");
          setSecret("");
          setBotName("");
          Alert.alert(copy.wecom.connected, copy.wecom.installSuccess);
        },
        onError: (error) => setFormError(wecomErrorMessage(error, copy.wecom.failed)),
      },
    );
  }, [botId, botName, canManage, copy.wecom.connected, copy.wecom.failed, copy.wecom.installSuccess, copy.wecom.required, register, secret, selectedAgentId]);

  const confirmDisconnect = useCallback(
    (installationId: string) => {
      if (!canManage) return;
      Alert.alert(
        copy.wecom.disconnectTitle,
        copy.wecom.disconnectDescription,
        [
          { text: copy.wecom.cancel, style: "cancel" },
          {
            text: copy.wecom.disconnect,
            style: "destructive",
            onPress: () =>
              disconnect.mutate(installationId, {
                onError: () =>
                  Alert.alert(copy.wecom.disconnect, copy.wecom.revokeFailed),
              }),
          },
        ],
      );
    },
    [canManage, copy.wecom.cancel, copy.wecom.disconnect, copy.wecom.disconnectDescription, copy.wecom.disconnectTitle, copy.wecom.revokeFailed, disconnect],
  );

  return (
    <>
      <Stack.Screen options={{ title: copy.wecom.title }} />
      <KeyboardAvoidingView
        className="flex-1 bg-background"
        behavior={Platform.OS === "ios" ? "padding" : undefined}
      >
        <ScrollView
          className="flex-1"
          contentContainerClassName="px-4 py-5 gap-6"
          keyboardShouldPersistTaps="handled"
        >
          {installationsQuery.isLoading ? (
            <View className="items-center py-8">
              <ActivityIndicator />
              <Text className="text-sm text-muted-foreground mt-3">
                {copy.wecom.loading}
              </Text>
            </View>
          ) : installationsQuery.error ? (
            <View className="gap-3">
              <Text className="text-sm text-destructive">
                {copy.wecom.failed}
              </Text>
              <Button
                variant="outline"
                onPress={() => void installationsQuery.refetch()}
              >
                <Text>{copy.channel.retry}</Text>
              </Button>
            </View>
          ) : !installationsQuery.data?.configured ? (
            <InfoCard
              title={copy.wecom.notEnabledTitle}
              description={copy.wecom.notEnabledDescription}
            />
          ) : (
            <>
              <InfoCard
                title={copy.wecom.previewTitle}
                description={copy.wecom.previewDescription}
              />

              <View className="gap-3">
                <Text className="text-xs uppercase tracking-wider text-muted-foreground">
                  {copy.wecom.connectedBots}
                </Text>
                {installations.length === 0 ? (
                  <InfoCard
                    title={copy.wecom.emptyTitle}
                    description={copy.wecom.emptyDescription}
                  />
                ) : (
                  <View className="rounded-md border border-border bg-card overflow-hidden">
                    {installations.map((installation, index) => (
                      <View key={installation.id}>
                        <View className="px-4 py-3 gap-1">
                          <View className="flex-row items-center gap-2">
                            <Text className="flex-1 text-base font-medium text-foreground">
                              {installation.bot_id}
                            </Text>
                            <Text className="text-xs text-muted-foreground">
                              {installation.status === "active"
                                ? copy.wecom.connected
                                : copy.wecom.revoked}
                            </Text>
                          </View>
                          <Text className="text-sm text-muted-foreground">
                            {agentName(agents, installation.agent_id)}
                          </Text>
                          {canManage && installation.status === "active" ? (
                            <Pressable
                              onPress={() => confirmDisconnect(installation.id)}
                              disabled={disconnect.isPending}
                              className="self-start pt-2"
                            >
                              <Text className="text-sm font-medium text-destructive">
                                {copy.wecom.disconnect}
                              </Text>
                            </Pressable>
                          ) : null}
                        </View>
                        {index < installations.length - 1 ? <Separator /> : null}
                      </View>
                    ))}
                  </View>
                )}
              </View>

              {installationsQuery.data.install_supported !== true ? (
                <InfoCard
                  title={copy.wecom.unsupportedTitle}
                  description={copy.wecom.unsupportedDescription}
                />
              ) : canManage ? (
                <View className="gap-4">
                  <Text className="text-xs uppercase tracking-wider text-muted-foreground">
                    {copy.wecom.connect}
                  </Text>
                  <Text className="text-sm text-muted-foreground">
                    {copy.wecom.connectHelp}
                  </Text>
                  <View className="gap-2">
                    <Text className="text-sm font-medium text-foreground">
                      {copy.wecom.selectAgent}
                    </Text>
                    <Pressable
                      onPress={chooseAgent}
                      className="flex-row items-center rounded-md border border-border bg-secondary/50 px-3 py-3"
                    >
                      <Text className="flex-1 text-sm text-foreground">
                        {selectedAgent?.name ?? copy.wecom.selectAgent}
                      </Text>
                      <Text className="text-sm text-muted-foreground">›</Text>
                    </Pressable>
                  </View>
                  <LabeledField label={copy.wecom.botId}>
                    <TextField
                      value={botId}
                      onChangeText={setBotId}
                      placeholder={copy.wecom.botId}
                      autoCapitalize="none"
                      autoCorrect={false}
                    />
                  </LabeledField>
                  <LabeledField label={copy.wecom.secret}>
                    <TextField
                      value={secret}
                      onChangeText={setSecret}
                      placeholder={copy.wecom.secret}
                      autoCapitalize="none"
                      autoCorrect={false}
                      secureTextEntry
                    />
                  </LabeledField>
                  <LabeledField label={copy.wecom.botName}>
                    <TextField
                      value={botName}
                      onChangeText={setBotName}
                      placeholder={copy.wecom.botNamePlaceholder}
                    />
                  </LabeledField>
                  {formError ? (
                    <Text className="text-sm text-destructive">{formError}</Text>
                  ) : null}
                  <Button
                    onPress={connect}
                    disabled={register.isPending || agentsQuery.isLoading}
                  >
                    <Text>
                      {register.isPending
                        ? copy.wecom.connecting
                        : copy.wecom.connect}
                    </Text>
                  </Button>
                </View>
              ) : !membersQuery.isLoading ? (
                <Text className="text-sm text-muted-foreground">
                  {copy.wecom.adminOnly}
                </Text>
              ) : null}
            </>
          )}
        </ScrollView>
      </KeyboardAvoidingView>
    </>
  );
}

function InfoCard({
  title,
  description,
}: {
  title: string;
  description: string;
}) {
  return (
    <View className="rounded-md border border-border bg-card px-4 py-4 gap-1">
      <Text className="text-base font-medium text-foreground">{title}</Text>
      <Text className="text-sm text-muted-foreground">{description}</Text>
    </View>
  );
}

function LabeledField({
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

function agentName(
  agents: Array<{ id: string; name: string }>,
  id: string,
): string {
  return agents.find((agent) => agent.id === id)?.name ?? id;
}

function wecomErrorMessage(error: unknown, fallback: string): string {
  if (!(error instanceof ApiError)) return fallback;
  const body = error.body;
  if (body && typeof body === "object" && "code" in body) {
    const code = String((body as { code: unknown }).code);
    if (
      code === "wecom_bot_owned_by_same_workspace" ||
      code === "wecom_bot_owned_by_archived_agent" ||
      code === "wecom_bot_owned_by_another_workspace"
    ) {
      return fallback;
    }
  }
  return fallback;
}
