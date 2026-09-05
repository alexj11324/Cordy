// @vitest-environment node
import { readFileSync, readdirSync } from "node:fs";
import { resolve, join } from "node:path";
import { describe, expect, it } from "vitest";

const root = resolve(process.cwd(), "../..");
function sources(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return sources(path);
    return /\.tsx?$/.test(path) && !/\.(test|spec)\./.test(path) ? [path] : [];
  });
}

describe("custom authentication UI contract", () => {
  it("does not import Clerk prebuilt sign-in or sign-up cards", () => {
    const files = ["apps/web/app", "apps/web/components", "apps/auth-broker/app", "apps/auth-broker/components", "packages/auth-ui"]
      .flatMap((directory) => sources(resolve(root, directory)));
    const forbidden = files.filter((file) =>
      /import\s*\{[^}]*\b(?:SignIn|SignUp)\b[^}]*\}\s*from\s*["']@clerk\//s.test(readFileSync(file, "utf8")),
    );
    expect(forbidden).toEqual([]);
  });
  it("has no prebuilt Clerk theme dependency or stylesheet", () => {
    const manifest = JSON.parse(readFileSync(resolve(root, "apps/web/package.json"), "utf8"));
    expect(manifest.dependencies).not.toHaveProperty("@clerk/themes");
    expect(readFileSync(resolve(root, "apps/web/app/globals.css"), "utf8")).not.toContain("@clerk/themes");
  });
});
