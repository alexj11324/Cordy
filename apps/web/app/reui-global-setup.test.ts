// @vitest-environment node
import { readFileSync } from "node:fs"
import { resolve } from "node:path"

import { describe, expect, it } from "vitest"

const repoRoot = resolve(process.cwd(), "../..")

function read(path: string): string {
  return readFileSync(resolve(repoRoot, path), "utf8")
}

function readJson(path: string): Record<string, unknown> {
  return JSON.parse(read(path)) as Record<string, unknown>
}

describe("ReUI global setup", () => {
  it("loads the shared ReUI theme after the legacy token layer", () => {
    const webCSS = read("apps/web/app/globals.css")
    const desktopCSS = read("apps/desktop/src/renderer/src/globals.css")
    const docsCSS = read("apps/docs/app/global.css")

    expect(webCSS.indexOf("styles/tokens.css")).toBeLessThan(
      webCSS.indexOf("styles/reui.css")
    )
    expect(desktopCSS).toContain('@import "@orvilo/ui/styles/reui.css";')
    expect(docsCSS).toContain(
      '@import "../../../packages/ui/styles/reui.css";'
    )
  })

  it("keeps the ReUI base-nova registry contract in sync", () => {
    const packageConfig = readJson("packages/ui/components.json")
    const webConfig = readJson("apps/web/components.json")

    expect(packageConfig.style).toBe("base-nova")
    expect(webConfig.style).toBe("base-nova")
    expect(
      (packageConfig.tailwind as Record<string, unknown>).css
    ).toBe("styles/reui.css")
    expect(
      (packageConfig.tailwind as Record<string, unknown>).baseColor
    ).toBe("neutral")
    expect(packageConfig.registries).toHaveProperty("@reui")
    expect(webConfig.registries).toHaveProperty("@reui")
  })

  it("maps ReUI semantic utilities without owning the product palette", () => {
    const reuiCSS = read("packages/ui/styles/reui.css")
    const tokensCSS = read("packages/ui/styles/tokens.css")

    expect(reuiCSS).toContain("--color-info: var(--info);")
    expect(reuiCSS).toContain("--color-success: var(--success);")
    expect(reuiCSS).toContain("--color-warning: var(--warning);")
    expect(reuiCSS).toContain("--color-invert: var(--invert);")
    expect(reuiCSS).toContain("--color-focus: var(--focus);")
    expect(reuiCSS).toContain("--color-focus-foreground: var(--focus-foreground);")
    expect(tokensCSS).toContain("--focus: var(--primary);")
    expect(tokensCSS).toContain("--focus-foreground: var(--primary-foreground);")
    expect(tokensCSS).toContain("--color-focus: var(--focus);")

    // Nova's chroma-0 literals used to live here and won at runtime because
    // this file loads after tokens.css.
    expect(reuiCSS).not.toMatch(/--background:\s*oklch\(/)
    expect(reuiCSS).not.toMatch(/--app-shell:\s*var\(--sidebar\)/)
    expect(tokensCSS).toContain("--background: var(--page-canvas);")
    expect(tokensCSS).toContain("--app-shell: oklch(0.979 0 0);")
    expect(tokensCSS).toContain("--page-canvas: oklch(1 0 0);")
    expect(tokensCSS).toContain("--muted-foreground: oklch(0.522 0 0);")
    expect(tokensCSS).toContain("--app-shell: oklch(0 0 0);")
    expect(tokensCSS).toContain("--page-canvas: oklch(0.159 0 0);")
    expect(tokensCSS).toContain("--sidebar: oklch(0 0 0);")
  })

  it("keeps the LobeHub color table (not layout) in tokens.css", () => {
    const tokensCSS = read("packages/ui/styles/tokens.css")
    const light = readBlock(tokensCSS, ":root")
    const dark = readBlock(tokensCSS, ".dark")

    // The palette is LobeHub's, not Pulse's. Every neutral is the value
    // `@lobehub/ui` resolves for the matching token in that appearance,
    // measured off a Lobe element in the running renderer and expressed as
    // oklch — Lobe's `colorFill*` overlays are composited onto the surface
    // they sit on, because the file's contract (and the contrast guard in
    // text-contrast.test.ts) is opaque oklch literals.
    // Aliases (--card: var(--surface)) resolve to the same literals.
    expectResolved(light, {
      "--background": "oklch(1 0 0)",
      "--foreground": "oklch(0.134 0 0)",
      "--card": "oklch(1 0 0)",
      "--card-foreground": "oklch(0.134 0 0)",
      "--popover": "oklch(1 0 0)",
      "--popover-foreground": "oklch(0.134 0 0)",
      "--primary": "oklch(0.205 0 0)",
      "--primary-foreground": "oklch(0.985 0 0)",
      "--secondary": "oklch(0.97 0 0)",
      "--secondary-foreground": "oklch(0.205 0 0)",
      "--muted": "oklch(0.976 0 0)",
      "--muted-foreground": "oklch(0.522 0 0)",
      "--accent": "oklch(0.955 0 0)",
      "--accent-foreground": "oklch(0.134 0 0)",
      "--destructive": "oklch(0.577 0.245 27.325)",
      "--border": "oklch(0.949 0 0)",
      "--input": "oklch(0.916 0 0)",
      "--ring": "oklch(0.708 0 0)",
      "--chart-1": "oklch(0.87 0 0)",
      "--chart-2": "oklch(0.556 0 0)",
      "--chart-3": "oklch(0.439 0 0)",
      "--chart-4": "oklch(0.371 0 0)",
      "--chart-5": "oklch(0.269 0 0)",
      "--sidebar": "oklch(0.979 0 0)",
      "--sidebar-foreground": "oklch(0.134 0 0)",
      "--sidebar-primary": "oklch(0.205 0 0)",
      "--sidebar-primary-foreground": "oklch(0.985 0 0)",
      "--sidebar-accent": "oklch(0.934 0 0)",
      "--sidebar-accent-foreground": "oklch(0.134 0 0)",
      "--sidebar-border": "oklch(0.949 0 0)",
      "--sidebar-ring": "oklch(0.708 0 0)",
      "--radius": "0.5rem",
    })

    expectResolved(dark, {
      "--background": "oklch(0.159 0 0)",
      "--foreground": "oklch(1 0 0)",
      "--card": "oklch(0.159 0 0)",
      "--card-foreground": "oklch(1 0 0)",
      "--popover": "oklch(0.218 0 0)",
      "--popover-foreground": "oklch(1 0 0)",
      "--primary": "oklch(0.922 0 0)",
      "--primary-foreground": "oklch(0.205 0 0)",
      "--secondary": "oklch(0.269 0 0)",
      "--secondary-foreground": "oklch(0.985 0 0)",
      "--muted": "oklch(0.226 0 0)",
      "--muted-foreground": "oklch(0.708 0 0)",
      "--accent": "oklch(0.264 0 0)",
      "--accent-foreground": "oklch(1 0 0)",
      "--destructive": "oklch(0.704 0.191 22.216)",
      "--border": "oklch(0.218 0 0)",
      "--input": "oklch(0.244 0 0)",
      "--ring": "oklch(0.556 0 0)",
      "--chart-1": "oklch(0.87 0 0)",
      "--chart-2": "oklch(0.556 0 0)",
      "--chart-3": "oklch(0.439 0 0)",
      "--chart-4": "oklch(0.371 0 0)",
      "--chart-5": "oklch(0.269 0 0)",
      // Lobe paints the chrome with `colorBgLayout` and the content with
      // `colorBgContainer`. This is the token both rails read: the shells no
      // longer rebind it, so the app rail and the portaled settings rail agree.
    })
  })

  /**
   * The shells used to rebind `--sidebar` to `--color-background`, plus the two
   * accents, to Pulse Help Desk's values — the rail painted the canvas colour on
   * purpose. That is no longer one skin: `tokens.css` carries LobeHub's palette
   * for these tokens app-wide, and the settings dialog portals out of the shell,
   * so a rebind here left the rail behind the dialog and the dialog's own rail
   * reading two different values. Measured before removing it: the app rail
   * painted `--color-background` (#ffffff) while the settings rail painted
   * `--sidebar` (#f8f8f8).
   *
   * This is a guard, not a description: re-adding the rebind re-opens the seam
   * silently, because each rail still looks internally consistent on its own.
   */
  it("leaves --sidebar to tokens.css in both shells, so the two rails agree", () => {
    const shells = {
      web: read("packages/views/layout/dashboard-layout.tsx"),
      desktop: read("apps/desktop/src/renderer/src/components/desktop-layout.tsx"),
    }

    for (const [name, source] of Object.entries(shells)) {
      expect(source, `${name} shell must not rebind --sidebar`).not.toMatch(
        /\[--sidebar:/,
      )
    }
  })
})

function readBlock(source: string, selector: string): Map<string, string> {
  const start = source.indexOf(`${selector} {`)
  if (start < 0) throw new Error(`${selector} block not found`)
  const end = source.indexOf("\n}", start)
  if (end < 0) throw new Error(`${selector} block is unterminated`)
  const declarations = new Map<string, string>()
  for (const match of source
    .slice(start, end)
    .matchAll(/(--[\w-]+):\s*([^;]+);/g)) {
    const [, name, value] = match
    if (name && value) declarations.set(name, value.trim())
  }
  return declarations
}

function resolveToken(
  declarations: Map<string, string>,
  name: string
): string {
  let current = name
  for (let hops = 0; hops < 8; hops++) {
    const value = declarations.get(current)
    if (value === undefined) {
      throw new Error(`${current} is not declared (while resolving ${name})`)
    }
    const alias = /^var\((--[\w-]+)\)$/.exec(value)?.[1]
    if (!alias) return value
    current = alias
  }
  throw new Error(`${name} did not resolve to a literal colour`)
}

function expectResolved(
  declarations: Map<string, string>,
  expected: Record<string, string>
) {
  for (const [name, value] of Object.entries(expected)) {
    expect(resolveToken(declarations, name), name).toBe(value)
  }
}
