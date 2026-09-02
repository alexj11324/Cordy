/**
 * Mobile Work Product entry point. `apps/mobile/CLAUDE.md` documents the v1
 * architecture as English-only and without a locale bundle, so these native
 * labels intentionally stay English while the shared Web/Desktop surface
 * uses all four product locales.
 */
import { useCallback, useMemo } from "react";
import { ActivityIndicator, FlatList, RefreshControl, View } from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";
import { useQuery } from "@tanstack/react-query";
import { Stack, router } from "expo-router";
import { Text } from "@/components/ui/text";
import { Button } from "@/components/ui/button";
import { WorkProductRow } from "@/components/work-product/work-product-row";
import { workProductListOptions } from "@/data/queries/work-products";
import { useWorkspaceStore } from "@/data/workspace-store";

export default function WorkProductsPage() {
  const wsId = useWorkspaceStore((state) => state.currentWorkspaceId);
  const wsSlug = useWorkspaceStore((state) => state.currentWorkspaceSlug);
  const { data = [], isLoading, error, refetch, isRefetching } = useQuery(
    workProductListOptions(wsId),
  );
  const products = useMemo(
    () => [...data].sort((a, b) => b.updated_at.localeCompare(a.updated_at)),
    [data],
  );
  const openDetail = useCallback(
    (id: string) => {
      if (wsSlug) router.push(`/${wsSlug}/more/work-products/${id}`);
    },
    [wsSlug],
  );

  return (
    <SafeAreaView className="flex-1 bg-background" edges={[]}>
      <Stack.Screen options={{ title: "Work Products" }} />
      {isLoading ? (
        <View className="flex-1 items-center justify-center">
          <ActivityIndicator />
        </View>
      ) : error ? (
        <View className="gap-3 px-4 pt-4">
          <Text className="text-sm text-destructive">
            Failed to load work products: {error instanceof Error ? error.message : "unknown error"}
          </Text>
          <Button variant="outline" onPress={() => void refetch()}>
            <Text>Retry</Text>
          </Button>
        </View>
      ) : products.length === 0 ? (
        <View className="flex-1 items-center justify-center px-6">
          <Text className="text-base font-medium text-foreground">No work products yet</Text>
        </View>
      ) : (
        <FlatList
          data={products}
          keyExtractor={(item) => item.id}
          ItemSeparatorComponent={() => <View className="ml-4 h-px bg-border" />}
          renderItem={({ item }) => (
            <WorkProductRow product={item} onPress={() => openDetail(item.id)} />
          )}
          refreshControl={<RefreshControl refreshing={isRefetching} onRefresh={refetch} />}
          contentContainerClassName="pb-6"
        />
      )}
    </SafeAreaView>
  );
}
