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
    expect(packageConfig.registries).toHaveProperty("@reui")
    expect(webConfig.registries).toHaveProperty("@reui")
  })

  it("provides ReUI core and extended semantic tokens in both themes", () => {
    const reuiCSS = read("packages/ui/styles/reui.css")

    expect(reuiCSS).toContain("--background: oklch(1 0 0);")
    expect(reuiCSS).toContain("--background: oklch(0.145 0 0);")
    expect(reuiCSS).toContain("--color-info: var(--info);")
    expect(reuiCSS).toContain("--color-success: var(--success);")
    expect(reuiCSS).toContain("--color-warning: var(--warning);")
    expect(reuiCSS).toContain("--color-invert: var(--invert);")
  })
})
