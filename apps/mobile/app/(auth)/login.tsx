import { useState } from "react";
import { KeyboardAvoidingView, Platform, View } from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";
import { router } from "expo-router";
import * as Haptics from "expo-haptics";
import { Text } from "@/components/ui/text";
import { TextField } from "@/components/ui/text-field";
import { Button } from "@/components/ui/button";
import { PatchbayLogo } from "@/components/brand/patchbay-logo";
import { useAuthStore } from "@/data/auth-store";
import { mapAuthError } from "@/lib/auth-error";

export default function Login() {
  const sendCode = useAuthStore((s) => s.sendCode);
  const continueAsGuest = useAuthStore((s) => s.continueAsGuest);
  const [email, setEmail] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [guestSubmitting, setGuestSubmitting] = useState(false);
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

  const onContinueAsGuest = async () => {
    if (guestSubmitting || submitting) return;
    void Haptics.selectionAsync();
    setGuestSubmitting(true);
    setError(null);
    try {
      await continueAsGuest();
      router.replace("/");
    } catch (err) {
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Error);
      setError(
        err instanceof Error
          ? err.message
          : "Couldn't start a guest session. Try again.",
      );
    } finally {
      setGuestSubmitting(false);
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
            <PatchbayLogo size={32} />
            <View className="gap-1 items-center">
              <Text className="text-2xl font-semibold text-foreground">
                Sign in to Patchbay
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

          <View className="items-center gap-3">
            <Text className="text-xs text-muted-foreground">or</Text>
            <Button
              size="lg"
              variant="outline"
              disabled={submitting || guestSubmitting}
              onPress={onContinueAsGuest}
            >
              <Text>
                {guestSubmitting ? "Starting guest session..." : "Continue as guest"}
              </Text>
            </Button>
          </View>
        </View>
      </KeyboardAvoidingView>
    </SafeAreaView>
  );
}
