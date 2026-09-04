// @vitest-environment node

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const css = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "globals.css"),
  "utf8",
);

function firstRule(selector: string): string {
  const start = css.indexOf(`${selector} {`);
  if (start < 0) {
    throw new Error(`missing ${selector}`);
  }
  const end = css.indexOf("\n}", start);
  return css.slice(start, end);
}

describe("accounts auth shell layout", () => {
  it("fills the viewport instead of sitting in a floating card", () => {
    const shell = firstRule(".accounts-auth-shell");

    expect(shell).toContain("min-height: 100dvh");
    expect(shell).not.toContain("border-radius");
    expect(shell).not.toContain("1500px");
    expect(shell).not.toContain("margin: 32px auto");
    expect(shell).not.toContain("border: 1px solid");
  });
});
