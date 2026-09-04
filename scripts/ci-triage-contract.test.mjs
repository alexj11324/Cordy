import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

async function workflow(name) {
  return readFile(new URL(`.github/workflows/${name}`, root), "utf8");
}

test("main CI applies the same validated path decisions after merge", async () => {
  const source = await workflow("ci.yml");

  assert.match(source, /base: \$\{\{ github\.ref \}\}/u);
  assert.match(source, /Test CI triage contract/u);
  assert.match(source, /frontend=\$FRONTEND_CHANGED/u);
  assert.match(source, /backend=\$BACKEND_CHANGED/u);
  assert.match(source, /sqlc=\$SQLC_CHANGED/u);
  assert.doesNotMatch(source, /EVENT_NAME/u);
  assert.doesNotMatch(source, /Always run everything on push to main/u);
});

test("mobile keeps its required check while gating expensive work", async () => {
  const source = await workflow("mobile-verify.yml");

  assert.match(source, /^  changes:\n/mu);
  assert.match(source, /^  mobile:\n    needs: changes\n    if: \$\{\{ !cancelled\(\) \}\}/mu);
  assert.match(source, /base: \$\{\{ github\.ref \}\}/u);
  assert.match(source, /- 'apps\/mobile\/\*\*'/u);
  assert.match(source, /- 'packages\/core\/\*\*'/u);
  assert.match(source, /CHANGES_RESULT: \$\{\{ needs\.changes\.result \}\}/u);

  for (const step of [
    "Checkout",
    "Setup pnpm",
    "Setup Node.js",
    "Install dependencies",
    "Type check, lint, and test",
  ]) {
    assert.match(
      source,
      new RegExp(
        `- name: ${step.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&")}\\n` +
          "\\s+if: \\$\\{\\{ needs\\.changes\\.outputs\\.mobile == 'true' \\}\\}",
        "u",
      ),
    );
  }
});
