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
    expect(tokensCSS).toContain("--app-shell: oklch(0.985 0 0);")
    expect(tokensCSS).toContain("--page-canvas: oklch(1 0 0);")
    expect(tokensCSS).toContain("--muted-foreground: oklch(0.53 0 0);")
    expect(tokensCSS).toContain("--app-shell: oklch(0.145 0 0);")
    expect(tokensCSS).toContain("--page-canvas: oklch(0.145 0 0);")
    expect(tokensCSS).toContain("--sidebar: oklch(0.145 0 0);")
  })

  it("keeps the Pulse Help Desk color table (not layout) in tokens.css", () => {
    const tokensCSS = read("packages/ui/styles/tokens.css")
    const light = readBlock(tokensCSS, ":root")
    const dark = readBlock(tokensCSS, ".dark")

    // Live Pulse Help Desk :root / .dark from pulse-helpdesk.reui.io.
    // Aliases (--card: var(--surface)) resolve to the same literals.
    expectResolved(light, {
      "--background": "oklch(1 0 0)",
      "--foreground": "oklch(0.145 0 0)",
      "--card": "oklch(1 0 0)",
      "--card-foreground": "oklch(0.145 0 0)",
      "--popover": "oklch(1 0 0)",
      "--popover-foreground": "oklch(0.145 0 0)",
      "--primary": "oklch(0.205 0 0)",
      "--primary-foreground": "oklch(0.985 0 0)",
      "--secondary": "oklch(0.97 0 0)",
      "--secondary-foreground": "oklch(0.205 0 0)",
      "--muted": "oklch(0.97 0 0)",
      // Pulse is 0.556; 0.53 is the WCAG AA floor on --surface-selected 0.95.
      "--muted-foreground": "oklch(0.53 0 0)",
      "--accent": "oklch(0.97 0 0)",
      "--accent-foreground": "oklch(0.205 0 0)",
      "--destructive": "oklch(0.577 0.245 27.325)",
      "--border": "oklch(0.922 0 0)",
      "--input": "oklch(0.922 0 0)",
      "--ring": "oklch(0.708 0 0)",
      "--chart-1": "oklch(0.87 0 0)",
      "--chart-2": "oklch(0.556 0 0)",
      "--chart-3": "oklch(0.439 0 0)",
      "--chart-4": "oklch(0.371 0 0)",
      "--chart-5": "oklch(0.269 0 0)",
      "--sidebar": "oklch(0.985 0 0)",
      "--sidebar-foreground": "oklch(0.145 0 0)",
      "--sidebar-primary": "oklch(0.205 0 0)",
      "--sidebar-primary-foreground": "oklch(0.985 0 0)",
      "--sidebar-accent": "oklch(0.97 0 0)",
      "--sidebar-accent-foreground": "oklch(0.205 0 0)",
      "--sidebar-border": "oklch(0.922 0 0)",
      "--sidebar-ring": "oklch(0.708 0 0)",
      "--radius": "0.625rem",
    })

    expectResolved(dark, {
      "--background": "oklch(0.145 0 0)",
      "--foreground": "oklch(0.985 0 0)",
      "--card": "oklch(0.205 0 0)",
      "--card-foreground": "oklch(0.985 0 0)",
      "--popover": "oklch(0.205 0 0)",
      "--popover-foreground": "oklch(0.985 0 0)",
      "--primary": "oklch(0.922 0 0)",
      "--primary-foreground": "oklch(0.205 0 0)",
      "--secondary": "oklch(0.269 0 0)",
      "--secondary-foreground": "oklch(0.985 0 0)",
      "--muted": "oklch(0.269 0 0)",
      "--muted-foreground": "oklch(0.708 0 0)",
      "--accent": "oklch(0.269 0 0)",
      "--accent-foreground": "oklch(0.985 0 0)",
      "--destructive": "oklch(0.704 0.191 22.216)",
      "--border": "oklch(1 0 0 / 10%)",
      "--input": "oklch(1 0 0 / 15%)",
      "--ring": "oklch(0.556 0 0)",
      "--chart-1": "oklch(0.87 0 0)",
      "--chart-2": "oklch(0.556 0 0)",
      "--chart-3": "oklch(0.439 0 0)",
      "--chart-4": "oklch(0.371 0 0)",
      "--chart-5": "oklch(0.269 0 0)",
      // Pulse's theme table sets --sidebar to 0.205, but the live shell
      // rebinds it to --background. We store the painted value so chrome
      // stays near-black even if the class override is dropped.
      "--sidebar": "oklch(0.145 0 0)",
      "--sidebar-foreground": "oklch(0.985 0 0)",
      "--sidebar-primary": "oklch(0.488 0.243 264.376)",
      "--sidebar-primary-foreground": "oklch(0.985 0 0)",
      "--sidebar-accent": "oklch(0.269 0 0)",
      "--sidebar-accent-foreground": "oklch(0.985 0 0)",
      "--sidebar-border": "oklch(1 0 0 / 10%)",
      "--sidebar-ring": "oklch(0.556 0 0)",
    })
  })

  it("applies Pulse Help Desk's sidebar rebind on web and desktop shells", () => {
    const web = read("packages/views/layout/dashboard-layout.tsx")
    const desktop = read(
      "apps/desktop/src/renderer/src/components/desktop-layout.tsx"
    )
    const rebind =
      "[--sidebar:var(--color-background)] [--sidebar-accent:color-mix(in_oklab,var(--color-primary)_5%,transparent)] [--sidebar-accent-foreground:var(--color-primary)]"

    expect(web).toContain(rebind)
    expect(desktop).toContain(rebind)
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
