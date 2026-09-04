import { useEffect, useRef, useState } from "react";
import { ActivityIndicator, View } from "react-native";
import { Button } from "@/components/ui/button";
import { Text } from "@/components/ui/text";
import { ApiError, api } from "@/data/api";
import { useAuthStore } from "@/data/auth-store";
import { getW8Copy } from "@/lib/w8-copy";
import { Stack, router, useLocalSearchParams } from "expo-router";
import { SafeAreaView } from "react-native-safe-area-context";

type BindStatus = "idle" | "needs-auth" | "redeeming" | "done" | "error";

export default function WecomBindPage() {
  const { token: rawToken } = useLocalSearchParams<{ token?: string | string[] }>();
  const token = Array.isArray(rawToken) ? rawToken[0] : rawToken;
  const user = useAuthStore((state) => state.user);
  const authLoading = useAuthStore((state) => state.isLoading);
  const copy = getW8Copy(user?.language);
  const [status, setStatus] = useState<BindStatus>("idle");
  const [message, setMessage] = useState<string | null>(null);
  const attemptedToken = useRef<string | null>(null);

  useEffect(() => {
    if (authLoading || !token) return;
    if (!user) {
      setStatus("needs-auth");
      return;
    }
    if (attemptedToken.current === token) return;
    attemptedToken.current = token;
    setStatus("redeeming");
    setMessage(null);
    void api
      .redeemWecomBindingToken(token)
      .then(() => setStatus("done"))
      .catch((error: unknown) => {
        setStatus("error");
        setMessage(bindErrorMessage(error, copy.bind));
      });
  }, [authLoading, copy.bind, token, user]);

  return (
    <SafeAreaView className="flex-1 bg-background">
      <Stack.Screen options={{ title: copy.bind.title, headerShown: true }} />
      <View className="flex-1 justify-center px-6 gap-5">
        {authLoading || status === "redeeming" ? (
          <View className="items-center gap-3">
            <ActivityIndicator />
            <Text className="text-sm text-muted-foreground text-center">
              {copy.bind.redeeming}
            </Text>
          </View>
        ) : !token ? (
          <Text className="text-base text-destructive text-center">
            {copy.bind.missingToken}
          </Text>
        ) : status === "needs-auth" ? (
          <View className="gap-4">
            <Text className="text-base text-foreground text-center">
              {copy.bind.signInRequired}
            </Text>
            <Button onPress={() => router.replace("/login")}>
              <Text>{copy.bind.signIn}</Text>
            </Button>
          </View>
        ) : status === "done" ? (
          <View className="gap-2">
            <Text className="text-xl font-semibold text-foreground text-center">
              {copy.bind.successTitle}
            </Text>
            <Text className="text-sm text-muted-foreground text-center">
              {copy.bind.successDescription}
            </Text>
          </View>
        ) : (
          <View className="gap-4">
            <Text className="text-base text-destructive text-center">
              {message ?? copy.bind.failed}
            </Text>
            <Button variant="outline" onPress={() => router.replace("/")}>
              <Text>{copy.bind.openAgain}</Text>
            </Button>
          </View>
        )}
      </View>
    </SafeAreaView>
  );
}

function bindErrorMessage(
  error: unknown,
  copy: ReturnType<typeof getW8Copy>["bind"],
): string {
  if (error instanceof ApiError) {
    if (error.status === 410) return copy.expired;
    if (error.status === 409) return copy.conflict;
    if (error.status === 403) return copy.notMember;
  }
  return copy.failed;
}
