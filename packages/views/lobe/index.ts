// Neutral home for the `@lobehub/ui` ↔ Orvilo seam.
//
// This lived under `chat/lobe/` while chat was the only antd surface. It moved
// here once the editor and settings page needed it too: those domains were
// reaching into chat for a theme bridge, which is a dependency direction that
// would have made `chat` a de-facto UI toolkit. Nothing in this folder knows
// about chat, and it is published as `@orvilo/views/lobe` for the apps.

export {
  LobeThemeBridge,
  type LobeThemeBridgeProps,
} from "./lobe-theme-bridge";
export {
  buildAntdTokens,
  createStaticTokenReader,
  type AntdMapTokens,
  type AntdSeedTokens,
  type OrviloAntdTokens,
  type OrviloTokenReader,
} from "./lobe-tokens";
export { oklchToHex, parseOklch, toAntdColor, type OklchColor } from "./oklch";
