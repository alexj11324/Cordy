/** See the parent Work Products screen for the English-only Mobile rationale. */
import { useCallback } from "react";
import { ActivityIndicator, Linking, Pressable, View } from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";
import { useQuery } from "@tanstack/react-query";
import { Stack, useLocalSearchParams } from "expo-router";
import { Text } from "@/components/ui/text";
import { Button } from "@/components/ui/button";
import { workProductDetailOptions } from "@/data/queries/work-products";
import { useWorkspaceStore } from "@/data/workspace-store";

export default function WorkProductDetailPage() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const productId = Array.isArray(id) ? id[0] ?? "" : id ?? "";
  const wsId = useWorkspaceStore((state) => state.currentWorkspaceId);
  const { data: product, isLoading, error, refetch } = useQuery(
    workProductDetailOptions(wsId, productId),
  );
  const openExternal = useCallback(() => {
    if (product?.external_url) void Linking.openURL(product.external_url);
  }, [product?.external_url]);

  return (
    <SafeAreaView className="flex-1 bg-background" edges={[]}>
      <Stack.Screen options={{ title: "Work Product" }} />
      {isLoading ? (
        <View className="flex-1 items-center justify-center">
          <ActivityIndicator />
        </View>
      ) : error || !product?.id ? (
        <View className="gap-3 px-4 pt-4">
          <Text className="text-sm text-destructive">Work product unavailable.</Text>
          <Button variant="outline" onPress={() => void refetch()}>
            <Text>Retry</Text>
          </Button>
        </View>
      ) : (
        <View className="gap-5 px-4 py-5">
          <View className="gap-1">
            <Text className="text-xl font-semibold text-foreground">
              {product.external_identity || "Work Product"}
            </Text>
            <Text className="text-sm text-muted-foreground">
              {product.provider || "Unknown provider"} · {product.kind || "Unknown kind"}
            </Text>
          </View>
          {product.external_url ? (
            <Pressable onPress={openExternal} className="active:opacity-70">
              <Text className="text-sm text-primary">Open external resource</Text>
            </Pressable>
          ) : null}
          <View className="gap-2 rounded-lg border border-border p-4">
            <Field label="External identity" value={product.external_identity} />
            <Field label="Provider" value={product.provider} />
            <Field label="Kind" value={product.kind} />
          </View>
        </View>
      )}
    </SafeAreaView>
  );
}

function Field({ label, value }: { label: string; value: string }) {
  return (
    <View className="gap-0.5">
      <Text className="text-xs text-muted-foreground">{label}</Text>
      <Text className="text-sm text-foreground" selectable>
        {value || "—"}
      </Text>
    </View>
  );
}
