import {
  configure,
  render,
  type RenderOptions,
  type RenderResult,
} from "@testing-library/react";
import { I18nProvider } from "@orvilo/core/i18n/react";
import { lazy, Suspense, type ReactElement, type ReactNode } from "react";
import { RESOURCES } from "../locales";
import type { SupportedLocale } from "@orvilo/core/i18n";

// Single i18n test wrapper for the whole package. Wraps the production
// `RESOURCES` map (every namespace registered there is available to the
// component under test) so when a new namespace lands the test never
// silently renders translation keys-as-text — the test sees the same
// resource set users do. The previous pattern of inlining a per-file
// `TEST_RESOURCES` slice meant every test author had to remember to
// extend the slice when their component started using a new namespace.
//
// Use `renderWithI18n` like the standard `render`. Pass `locale: "zh-Hans"`,
// `locale: "ko"`, or `locale: "ja"` to verify localized strings; default is "en".
type RenderArgs = Omit<RenderOptions, "wrapper"> & {
  locale?: SupportedLocale;
  /** Mount the Lobe theme bridge. Required for anything from `@lobehub/ui`. */
  lobe?: boolean;
};

// Loaded on first use, never at module scope. The bridge reaches antd, the
// `@lobehub/ui/base-ui` barrel and `motion/react` — measured at ~3-4s of module
// graph *per test file*, because Vitest gives every file its own module
// registry. 83 files call this helper and a handful render Lobe components, so
// a static import would charge all of them for the few: on a six-suite sample
// it took the run from 15.1s to 20.7s. `lazy` pays that once, only in files
// that ask for it, and caches the resolved component for the rest of the file.
//
// Consequence for callers: the first query in an opted-in test must be an
// async one (`await screen.findBy…`), and it waits on that import.
const LobeBridge = lazy(async () => {
  const module = await import("../lobe");
  return { default: module.LobeThemeBridge };
});

/**
 * How long an async query waits in an opted-in file. Testing Library defaults
 * to 1s, which the on-demand bridge import above routinely exceeds on a loaded
 * machine; in an opted-in file the first `findBy` is really waiting for a
 * module, not for a render. `configure` is per test file — Vitest gives each
 * file its own module registry — so this only widens the budget where the
 * bridge is actually used.
 */
const BRIDGE_LOAD_TIMEOUT_MS = 10_000;

export function renderWithI18n(
  ui: ReactElement,
  options: RenderArgs = {},
): RenderResult {
  const { locale = "en", lobe = false, ...rest } = options;
  if (lobe) configure({ asyncUtilTimeout: BRIDGE_LOAD_TIMEOUT_MS });
  function Wrapper({ children }: { children: ReactNode }) {
    const content = (
      <I18nProvider locale={locale} resources={RESOURCES}>
        {children}
      </I18nProvider>
    );
    if (!lobe) return content;
    return (
      <Suspense fallback={null}>
        <LobeBridge>{content}</LobeBridge>
      </Suspense>
    );
  }
  return render(ui, { wrapper: Wrapper, ...rest });
}

export { RESOURCES };
