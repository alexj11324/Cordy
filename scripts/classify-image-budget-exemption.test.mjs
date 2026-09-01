import assert from "node:assert/strict";
import test from "node:test";

import { hasImageBudgetExemption } from "./classify-image-budget-exemption.mjs";

test("recognizes an exemption line with a non-empty reason", () => {
  assert.equal(
    hasImageBudgetExemption(["Issue summary\nOversized image exemption: product screenshot"]),
    true,
  );
});

test("does not treat an empty exemption line as an approval", () => {
  assert.equal(hasImageBudgetExemption(["Oversized image exemption:"]), false);
});

test("preserves an exemption from any constituent merge-group PR", () => {
  assert.equal(
    hasImageBudgetExemption(["first PR", "Oversized image exemption: required artwork"]),
    true,
  );
});
