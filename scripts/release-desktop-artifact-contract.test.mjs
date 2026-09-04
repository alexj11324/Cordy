import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { basename } from "node:path";
import test from "node:test";

test("desktop release staging selects installers and updater metadata only", async () => {
  const workflow = await readFile(
    new URL("../.github/workflows/release.yml", import.meta.url),
    "utf8",
  );
  const literal = workflow.match(/const assetPattern = (\/[^\n]+\/i);/u)?.[1];
  assert.ok(literal, "release workflow must declare its desktop asset filter");

  const assetPattern = Function(`"use strict"; return ${literal};`)();
  const candidates = [
    "win-x64/win-unpacked/Patchbay.exe",
    "win-x64/win-unpacked/resources/elevate.exe",
    "win-arm64/win-arm64-unpacked/Patchbay.exe",
    "win-x64/patchbay-desktop-0.2.7-windows-x64.__uninstaller.exe",
    "win-x64/patchbay-desktop-0.2.7-windows-x64.exe",
    "win-x64/patchbay-desktop-0.2.7-windows-x64.exe.blockmap",
    "win-x64/latest.yml",
    "win-arm64/patchbay-desktop-0.2.7-windows-arm64.exe",
    "win-arm64/patchbay-desktop-0.2.7-windows-arm64.exe.blockmap",
    "win-arm64/latest-arm64.yml",
  ];

  const selected = candidates
    .map((path) => basename(path))
    .filter((name) => assetPattern.test(name))
    .sort();

  assert.deepEqual(selected, [
    "latest-arm64.yml",
    "latest.yml",
    "patchbay-desktop-0.2.7-windows-arm64.exe",
    "patchbay-desktop-0.2.7-windows-arm64.exe.blockmap",
    "patchbay-desktop-0.2.7-windows-x64.exe",
    "patchbay-desktop-0.2.7-windows-x64.exe.blockmap",
  ]);
  assert.equal(new Set(selected).size, selected.length);
});
