import { describe, expect, it } from "vitest";
import {
  ACP_LOBEHUB_ICON_BY_PROVIDER,
  ALL_RUNTIME_PROVIDERS,
  acpAvatarSource,
  resolveAgentProvider,
} from "./acp-avatar-map";

describe("acpAvatarSource", () => {
  it("maps every shipping runtime onto a logo source", () => {
    for (const provider of ALL_RUNTIME_PROVIDERS) {
      expect(acpAvatarSource(provider).kind).not.toBe("none");
    }
  });

  it("uses the matching @lobehub/icons Avatar, not AgentIcon keyword matching", () => {
    expect(acpAvatarSource("claude")).toEqual({
      kind: "lobehub",
      id: "ClaudeCode",
      provider: "claude",
    });
    expect(acpAvatarSource("codebuddy")).toEqual({
      kind: "lobehub",
      id: "CodeBuddy",
      provider: "codebuddy",
    });
    expect(acpAvatarSource("codex")).toEqual({
      kind: "lobehub",
      id: "Codex",
      provider: "codex",
    });
    expect(acpAvatarSource("cursor")).toEqual({
      kind: "lobehub",
      id: "Cursor",
      provider: "cursor",
    });
    // AgentIcon would send kiro → KiloCode. The explicit map must not.
    expect(acpAvatarSource("kiro")).toEqual({
      kind: "lobehub",
      id: "Kiro",
      provider: "kiro",
    });
    expect(acpAvatarSource("qoderclicn")).toEqual({
      kind: "lobehub",
      id: "Qoder",
      provider: "qoderclicn",
    });
    expect(acpAvatarSource("omp")).toEqual({
      kind: "lobehub",
      id: "Pi",
      provider: "omp",
    });
    expect(acpAvatarSource("deveco")).toEqual({
      kind: "lobehub",
      id: "Huawei",
      provider: "deveco",
    });
    expect(acpAvatarSource("qwenpaw")).toEqual({
      kind: "lobehub",
      id: "Qwen",
      provider: "qwenpaw",
    });
  });

  it("falls back to ProviderLogo for runtimes lobehub does not ship", () => {
    expect(acpAvatarSource("reasonix")).toEqual({
      kind: "fallback",
      provider: "reasonix",
    });
    expect(acpAvatarSource("dim")).toEqual({
      kind: "fallback",
      provider: "dim",
    });
    expect(acpAvatarSource("unknown-runtime")).toEqual({
      kind: "fallback",
      provider: "unknown-runtime",
    });
  });

  it("has no face before a runtime is chosen", () => {
    expect(acpAvatarSource(null)).toEqual({ kind: "none" });
    expect(acpAvatarSource(undefined)).toEqual({ kind: "none" });
    expect(acpAvatarSource("")).toEqual({ kind: "none" });
  });

  it("covers every provider that has a lobehub mark", () => {
    const mapped = Object.keys(ACP_LOBEHUB_ICON_BY_PROVIDER);
    expect(mapped.sort()).toEqual(
      ALL_RUNTIME_PROVIDERS.filter(
        (provider) => provider !== "reasonix" && provider !== "dim",
      ).slice().sort(),
    );
  });
});

describe("resolveAgentProvider", () => {
  it("follows agent.runtime_id onto the runtime provider", () => {
    expect(
      resolveAgentProvider(
        "agent-1",
        [{ id: "agent-1", runtime_id: "rt-codex" }],
        [
          { id: "rt-codex", provider: "codex" },
          { id: "rt-kiro", provider: "kiro" },
        ],
      ),
    ).toBe("codex");
  });

  it("returns null when the agent has no bound runtime", () => {
    expect(
      resolveAgentProvider(
        "agent-1",
        [{ id: "agent-1", runtime_id: "missing" }],
        [{ id: "rt-codex", provider: "codex" }],
      ),
    ).toBeNull();
    expect(resolveAgentProvider(undefined, [], [])).toBeNull();
  });
});
