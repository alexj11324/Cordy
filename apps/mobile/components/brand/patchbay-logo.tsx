/**
 * Patchbay routing mark. Keep this geometry in sync with
 * docs/assets/brand/patchbay/mark-color.svg.
 *
 * react-native-svg does not resolve CSS `currentColor`, so callers must pass
 * `color` explicitly. For theme-aware usage, pair with `useColorScheme` +
 * `THEME` token from `@/lib/theme`.
 */
import Svg, { Circle, G, Path } from "react-native-svg";
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
      <G fill="none" stroke={resolvedColor} strokeWidth={8}>
        <Circle cx={64} cy={28} r={10} />
        <Circle cx={100} cy={28} r={10} />
        <Circle cx={28} cy={64} r={10} />
        <Circle cx={100} cy={64} r={10} />
        <Circle cx={28} cy={100} r={10} />
        <Circle cx={64} cy={100} r={10} />
      </G>
      <G
        fill="none"
        stroke="#B6F000"
        strokeWidth={8}
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <Circle cx={28} cy={28} r={10} />
        <Path d="M28 38C28 46 34 50 42 50H50C60 50 64 54 64 64V72C64 82 68 86 78 86H82C92 86 96 90 96 100" />
        <Circle cx={100} cy={100} r={10} />
      </G>
    </Svg>
  );
}
