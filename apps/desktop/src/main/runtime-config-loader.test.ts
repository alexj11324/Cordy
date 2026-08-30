// @vitest-environment node
import { mkdtemp, writeFile } from "fs/promises";
import { join } from "path";
import { tmpdir } from "os";
import { describe, expect, it } from "vitest";
import { loadRuntimeConfig } from "./runtime-config-loader";

describe("loadRuntimeConfig", () => {
  it("uses dev env and ignores desktop.json during electron-vite dev", async () => {
    const dir = await mkdtemp(join(tmpdir(), "patchbay-desktop-config-"));
    const configPath = join(dir, "desktop.json");
    await writeFile(
      configPath,
      JSON.stringify({ schemaVersion: 1, apiUrl: "https://prod.example.com" }),
    );

    await expect(
      loadRuntimeConfig({
        isDev: true,
        configPath,
        env: {
          apiUrl: "http://localhost:8080",
          wsUrl: "ws://localhost:8080/ws",
          appUrl: "http://localhost:3000",
        },
      }),
    ).resolves.toEqual({
      ok: true,
      config: {
        schemaVersion: 1,
        apiUrl: "http://localhost:8080",
        wsUrl: "ws://localhost:8080/ws",
        appUrl: "http://localhost:3000",
        accountsUrl: "http://localhost:3000",
      },
    });
  });

  it("uses cloud defaults when packaged config is absent", async () => {
    const dir = await mkdtemp(join(tmpdir(), "patchbay-desktop-config-"));
    await expect(
      loadRuntimeConfig({
        isDev: false,
        configPath: join(dir, "missing.json"),
        env: {},
      }),
    ).resolves.toEqual({
      ok: true,
      config: {
        schemaVersion: 1,
        apiUrl: "https://api.aspectlylabs.com",
        wsUrl: "wss://api.aspectlylabs.com/ws",
        appUrl: "https://patchbay.aspectlylabs.com",
        accountsUrl: "https://accounts.aspectlylabs.com",
      },
    });
  });

  it("ignores the built-in localhost dev tuple in packaged mode", async () => {
    const dir = await mkdtemp(join(tmpdir(), "patchbay-desktop-config-"));
    const configPath = join(dir, "desktop.json");
    await writeFile(
      configPath,
      JSON.stringify({
        schemaVersion: 1,
        apiUrl: "http://localhost:8080",
        wsUrl: "ws://localhost:8080/ws",
        appUrl: "http://localhost:3000",
      }),
    );

    await expect(
      loadRuntimeConfig({ isDev: false, configPath, env: {} }),
    ).resolves.toEqual({
      ok: true,
      config: {
        schemaVersion: 1,
        apiUrl: "https://api.aspectlylabs.com",
        wsUrl: "wss://api.aspectlylabs.com/ws",
        appUrl: "https://patchbay.aspectlylabs.com",
        accountsUrl: "https://accounts.aspectlylabs.com",
      },
    });
  });

  it("repairs a stale localhost accounts origin for the managed packaged API", async () => {
    const dir = await mkdtemp(join(tmpdir(), "patchbay-desktop-config-"));
    const configPath = join(dir, "desktop.json");
    await writeFile(
      configPath,
      JSON.stringify({
        schemaVersion: 1,
        apiUrl: "https://api.aspectlylabs.com",
        wsUrl: "wss://api.aspectlylabs.com/ws",
        appUrl: "https://patchbay.aspectlylabs.com",
        accountsUrl: "http://localhost:3000",
      }),
    );

    await expect(
      loadRuntimeConfig({ isDev: false, configPath, env: {} }),
    ).resolves.toEqual({
      ok: true,
      config: {
        schemaVersion: 1,
        apiUrl: "https://api.aspectlylabs.com",
        wsUrl: "wss://api.aspectlylabs.com/ws",
        appUrl: "https://patchbay.aspectlylabs.com",
        accountsUrl: "https://accounts.aspectlylabs.com",
      },
    });
  });

  it("preserves explicit self-hosted packaged runtime URLs", async () => {
    const dir = await mkdtemp(join(tmpdir(), "patchbay-desktop-config-"));
    const configPath = join(dir, "desktop.json");
    await writeFile(
      configPath,
      JSON.stringify({
        schemaVersion: 1,
        apiUrl: "https://api.example.com",
        appUrl: "https://app.example.com",
        wsUrl: "wss://ws.example.com/socket",
        accountsUrl: "https://app.example.com",
      }),
    );

    await expect(
      loadRuntimeConfig({ isDev: false, configPath, env: {} }),
    ).resolves.toEqual({
      ok: true,
      config: {
        schemaVersion: 1,
        apiUrl: "https://api.example.com",
        appUrl: "https://app.example.com",
        wsUrl: "wss://ws.example.com/socket",
        accountsUrl: "https://app.example.com",
      },
    });
  });

  it("parses a valid packaged desktop.json", async () => {
    const dir = await mkdtemp(join(tmpdir(), "patchbay-desktop-config-"));
    const configPath = join(dir, "desktop.json");
    await writeFile(
      configPath,
      JSON.stringify({ schemaVersion: 1, apiUrl: "https://api.example.com" }),
    );

    await expect(
      loadRuntimeConfig({ isDev: false, configPath, env: {} }),
    ).resolves.toEqual({
      ok: true,
      config: {
        schemaVersion: 1,
        apiUrl: "https://api.example.com",
        wsUrl: "wss://api.example.com/ws",
        appUrl: "https://example.com",
        accountsUrl: "https://example.com",
      },
    });
  });

  it("fails closed when packaged desktop.json is invalid", async () => {
    const dir = await mkdtemp(join(tmpdir(), "patchbay-desktop-config-"));
    const configPath = join(dir, "desktop.json");
    await writeFile(configPath, "{");

    const result = await loadRuntimeConfig({ isDev: false, configPath, env: {} });

    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.error.message).toContain(configPath);
      expect(result.error.message).toContain("Invalid desktop runtime config JSON");
    }
  });
});
