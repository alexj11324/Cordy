import { useState } from "react";
import { KeyboardAvoidingView, Platform, View } from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";
import { router } from "expo-router";
import * as Haptics from "expo-haptics";
import { Ionicons } from "@expo/vector-icons";
import { useAuth } from "@clerk/expo";
import { useSSO } from "@clerk/expo/experimental";
import { Text } from "@/components/ui/text";
import { TextField } from "@/components/ui/text-field";
import { Button } from "@/components/ui/button";
import { OrviloLogo } from "@/components/brand/orvilo-logo";
import { useAuthStore } from "@/data/auth-store";
import { mapAuthError } from "@/lib/auth-error";

const clerkConfigured = Boolean(
  process.env.EXPO_PUBLIC_CLERK_PUBLISHABLE_KEY?.trim(),
);

export default function Login() {
  const sendCode = useAuthStore((s) => s.sendCode);
  const [email, setEmail] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const onSubmit = async () => {
    const trimmed = email.trim();
    if (!trimmed) return;
    void Haptics.selectionAsync();
    setSubmitting(true);
    setError(null);
    try {
      await sendCode(trimmed);
      router.push({ pathname: "/verify", params: { email: trimmed } });
    } catch (err) {
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Error);
      setError(mapAuthError(err, "Couldn't send the code. Try again."));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <SafeAreaView className="flex-1 bg-background">
      <KeyboardAvoidingView
        className="flex-1"
        behavior={Platform.OS === "ios" ? "padding" : undefined}
      >
        <View className="flex-1 justify-center px-6 gap-6">
          <View className="items-center gap-3">
            <OrviloLogo size={32} />
            <View className="gap-1 items-center">
              <Text className="text-2xl font-semibold text-foreground">
                Sign in to Orvilo
              </Text>
              <Text className="text-sm text-muted-foreground text-center">
                Enter your email and we&apos;ll send you a verification code.
              </Text>
            </View>
          </View>

          <View className="gap-3">
            <TextField
              autoCapitalize="none"
              autoComplete="email"
              autoFocus
              keyboardType="email-address"
              placeholder="you@example.com"
              value={email}
              onChangeText={setEmail}
              onSubmitEditing={onSubmit}
              returnKeyType="send"
              editable={!submitting}
              invalid={!!error}
            />
            {error ? (
              <Text className="text-sm text-destructive">{error}</Text>
            ) : null}
          </View>

          <Button
            size="lg"
            disabled={submitting || !email.trim()}
            onPress={onSubmit}
          >
            <Text>{submitting ? "Sending..." : "Send code"}</Text>
          </Button>

          {clerkConfigured ? (
            <>
              <View className="flex-row items-center gap-3">
                <View className="h-px flex-1 bg-border" />
                <Text className="text-sm text-muted-foreground">or</Text>
                <View className="h-px flex-1 bg-border" />
              </View>
              <GoogleLoginButton disabled={submitting} />
            </>
          ) : null}

        </View>
      </KeyboardAvoidingView>
    </SafeAreaView>
  );
}

function GoogleLoginButton({ disabled }: { disabled: boolean }) {
  const { startSSOFlow } = useSSO();
  const { getToken } = useAuth();
  const signInWithClerkToken = useAuthStore(
    (state) => state.signInWithClerkToken,
  );
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const onPress = async () => {
    if (disabled || submitting) return;
    void Haptics.selectionAsync();
    setSubmitting(true);
    setError(null);
    try {
      const result = await startSSOFlow({
        strategy: "oauth_google",
        oidcPrompt: "select_account",
      });

      // Clerk returns a non-success result for a user-cancelled browser
      // session. Leave the form usable without showing a false auth error.
      if (result.authSessionResult?.type !== "success") return;

      const clerkSessionToken = await getToken();
      if (!clerkSessionToken) {
        throw new Error("Clerk session token unavailable");
      }
      await signInWithClerkToken(clerkSessionToken);
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
      router.replace("/");
    } catch (err) {
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Error);
      setError(mapAuthError(err, "Couldn't sign in with Google. Try again."));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <View className="gap-3">
      <Button
        size="lg"
        variant="outline"
        disabled={disabled || submitting}
        onPress={onPress}
      >
        <Ionicons name="logo-google" size={18} color="#4285F4" />
        <Text>
          {submitting ? "Opening Google..." : "Continue with Google"}
        </Text>
      </Button>
      {error ? (
        <Text className="text-sm text-destructive text-center">{error}</Text>
      ) : null}
    </View>
  );
}
