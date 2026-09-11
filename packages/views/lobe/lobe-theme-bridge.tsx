"use client";

/**
 * Keeps `@lobehub/ui` / antd components looking like the rest of Orvilo.
 *
 * Surfaces built from antd-based components (chat, the feedback editor, and
 * the settings page) sit inside an app that is Tailwind over the CSS variables
 * in `packages/ui/styles/tokens.css`. This bridge is the only place the two
 * systems meet: it reads those variables in the browser and hands antd the
 * matching token set.
 *
 * Four decisions worth knowing about:
 *
 * - `cssVar.key` is namespaced to Orvilo rather than reusing LobeHub's
 *   `lobe-vars`. The published packages reference their CSS variables only
 *   through antd's `cssVar` API, so the key is ours to choose, and a private
 *   namespace cannot collide with anything LobeHub ships later.
 *
 * - Tokens are re-read whenever `<html>`'s class changes. next-themes flips
 *   dark mode by swapping that class, without remounting the tree, so a
 *   one-shot read on mount would leave the panel in the wrong theme after a
 *   toggle. The read itself is synchronous and cheap.
 *
 * - `MotionProvider` is mandatory, not decorative. `ThemeProvider` below does
 *   not supply a motion context, and every `base-ui` component that animates —
 *   `Button`, `Form.SubmitFooter`, the `Modal` and `Drawer` atoms — calls
 *   `useMotionComponent()`, which throws without one. Mounting the theme
 *   provider alone renders a tree that dies on the first button. It is a named
 *   export (the module has no default), and the component it takes must be
 *   `motion` from `motion/react` — the same one `@lobehub/ui` itself renders
 *   through.
 *
 * - `enableGlobalStyle` is turned off. It defaults to on, and when on it
 *   injects a document-wide reset (`body { margin: 0; line-height: 1;
 *   overflow: hidden auto; background-color: ... }`, `html { overscroll-behavior:
 *   none }`). That is defensible for a full-page LobeHub app; it is not for a
 *   bridge mounted in a panel, a modal, or a dashboard route, where it would
 *   restyle and rescroll the page behind it.
 */

import { StyleProvider } from "@ant-design/cssinjs";
// Deep imports on purpose. The package root re-exports every component it
// ships, and anything that pulls it in drags the whole library into the module
// graph — under Vitest that evaluates SortableList, which needs a
// `defaultDropAnimationSideEffects` export our dnd-kit mocks do not provide,
// breaking seven unrelated issue-board suites. `ThemeProvider` has its own
// entry and reaches only four internal modules, and `MotionProvider` is a
// context-only module of a few lines, so this also keeps a large amount of
// dead weight out of the renderer bundle.
import { MotionProvider } from "@lobehub/ui/es/MotionProvider/index";
import ThemeProvider from "@lobehub/ui/es/ThemeProvider/index";
import { motion } from "motion/react";
import { type ReactNode, useEffect, useState } from "react";

import {
  buildAntdTokens,
  createStaticTokenReader,
  type OrviloAntdTokens,
  type OrviloTokenReader,
} from "./lobe-tokens";

/** The class next-themes toggles on `<html>` for dark mode. */
const DARK_CLASS = "dark";

/** antd variables are namespaced under this key. */
const CSS_VAR_KEY = "orvilo-lobe";

/**
 * antd wants unitless pixels, but our radius tokens are authored in rem
 * (`--radius: 0.625rem`). Converting needs the live root font size rather than
 * a hardcoded 16, because the app scales text per locale and per user setting.
 */
const FALLBACK_ROOT_FONT_PX = 16;

function rootFontSizePx(): number {
  const parsed = Number.parseFloat(getComputedStyle(document.documentElement).fontSize);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : FALLBACK_ROOT_FONT_PX;
}

/**
 * Reads design tokens off the live document, degrading to the static defaults
 * for any token it cannot resolve.
 *
 * The fallback is load-bearing, not belt-and-braces: `@lobehub/ui`'s
 * ThemeProvider derives a dozen extra tokens by running `polished.mix()` over
 * the ones we hand it (`colorBgContainerSecondary` from `colorBgLayout` and
 * `colorBgContainer`, for one). An empty string reaches that colour maths and
 * throws, taking the entire panel down — so a token that is unset, misspelled,
 * or simply unreadable must never be passed through as "".
 */
function createDomTokenReader(): OrviloTokenReader {
  const fallback = createStaticTokenReader();

  const read = (token: string): string => {
    const value = getComputedStyle(document.documentElement).getPropertyValue(token).trim();
    return value || fallback.color(token);
  };

  return {
    color: read,
    font: read,
    lengthPx: (token) => {
      const raw = read(token);
      const value = Number.parseFloat(raw);
      if (!Number.isFinite(value)) return fallback.lengthPx(token);
      return raw.endsWith("rem") ? value * rootFontSizePx() : value;
    },
  };
}

interface BridgeTheme {
  appearance: "light" | "dark";
  tokens: OrviloAntdTokens;
}

function readBridgeTheme(): BridgeTheme {
  return {
    appearance: document.documentElement.classList.contains(DARK_CLASS) ? "dark" : "light",
    tokens: buildAntdTokens(createDomTokenReader()),
  };
}

export interface LobeThemeBridgeProps {
  children: ReactNode;
}

export function LobeThemeBridge({ children }: LobeThemeBridgeProps) {
  // Seeded from the static reader so the server render and the first client
  // paint agree; the effect below replaces it with the real values.
  const [theme, setTheme] = useState<BridgeTheme>(() => ({
    appearance: "light",
    tokens: buildAntdTokens(createStaticTokenReader()),
  }));

  useEffect(() => {
    const sync = () => setTheme(readBridgeTheme());
    sync();

    const observer = new MutationObserver(sync);
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class"],
    });
    return () => observer.disconnect();
  }, []);

  return (
    // Renders no DOM of its own, so it can wrap the whole bridge without
    // affecting the layout decision below.
    <MotionProvider motion={motion}>
      <StyleProvider hashPriority="low">
        <ThemeProvider
          appearance={theme.appearance}
          // Document-wide body/html reset; see the header note on why a bridge
          // must not install one.
          enableGlobalStyle={false}
          // `ThemeProvider` renders a real <div>. Callers mount this bridge
          // inside flex columns (the message list sits between a scroll
          // container and the composer), where an extra box would take a share
          // of the layout. `display: contents` removes the box while the CSS
          // variables still inherit down to the panel.
          className="contents"
          theme={{
            cssVar: { key: CSS_VAR_KEY },
            token: { ...theme.tokens.seed, ...theme.tokens.map },
          }}
        >
          {children}
        </ThemeProvider>
      </StyleProvider>
    </MotionProvider>
  );
}
