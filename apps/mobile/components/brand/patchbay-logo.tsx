/**
 * Orvilo brand mark. Keep this geometry in sync with
 * docs/assets/brand/orvilo/mark.svg.
 *
 * react-native-svg does not resolve CSS `currentColor`, so callers must pass
 * `color` explicitly. For theme-aware usage, pair with `useColorScheme` +
 * `THEME` token from `@/lib/theme`.
 */
import Svg, { Path } from "react-native-svg";
import { THEME } from "@/lib/theme";
import { useColorScheme } from "@/lib/use-color-scheme";

interface PatchbayLogoProps {
  size?: number;
  color?: string;
}

export function PatchbayLogo({ size = 48, color }: PatchbayLogoProps) {
  const { isDarkColorScheme } = useColorScheme();
  const resolvedColor =
    color ?? (isDarkColorScheme ? THEME.dark.foreground : THEME.light.foreground);

  return (
    <Svg width={size} height={size} viewBox="0 0 128 128">
      <Path fill={resolvedColor} d="M47 39C48 26 56 20 68 20H85C100 20 108 28 108 43V64C108 79 99 87 86 87H67C79 79 85 70 85 59C85 47 77 39 64 39Z" />
      <Path fill={resolvedColor} d="M81 89C80 102 72 108 60 108H43C28 108 20 100 20 85V64C20 49 29 41 42 41H61C49 49 43 58 43 69C43 81 51 89 64 89Z" />
    </Svg>
  );
}
