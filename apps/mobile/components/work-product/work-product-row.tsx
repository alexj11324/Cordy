/**
 * Native list row for Work Products. The data shape and identity mirror the
 * shared Web/Desktop row; only the container differs because Mobile uses the
 * existing iOS Pressable + FlatList pattern. The right-side metadata stacks
 * vertically to keep long provider identities from colliding with the date.
 */
import { Pressable, View } from "react-native";
import { Ionicons } from "@expo/vector-icons";
import { useTheme } from "@react-navigation/native";
import type { WorkProduct } from "@patchbay/core/types";
import { Text } from "@/components/ui/text";
import { timeAgo } from "@/lib/time-ago";

export function WorkProductRow({
  product,
  onPress,
}: {
  product: WorkProduct;
  onPress: () => void;
}) {
  const { colors } = useTheme();
  return (
    <Pressable onPress={onPress} className="active:bg-secondary px-4 py-3">
      <View className="flex-row items-start gap-3">
        <Ionicons
          name="document-text-outline"
          size={26}
          color={colors.text}
          accessibilityLabel=""
        />
        <View className="flex-1 min-w-0 gap-1">
          <Text className="text-sm text-foreground font-medium" numberOfLines={1}>
            {product.external_identity || "Unknown work product"}
          </Text>
          <Text className="text-xs text-muted-foreground" numberOfLines={1}>
            {product.provider || "Unknown provider"} · {product.kind || "Unknown kind"}
          </Text>
        </View>
        <View className="items-end gap-1">
          <Ionicons
            name="chevron-forward"
            size={16}
            color={colors.text}
            accessibilityLabel=""
          />
          <Text className="text-[11px] text-muted-foreground/70">
            {timeAgo(product.updated_at)}
          </Text>
        </View>
      </View>
    </Pressable>
  );
}
