// @vitest-environment node
import { describe, expect, it } from "vitest";
import {
  deviceDisplayName,
  deviceKind,
  deviceLabelForViewer,
  isCurrentLocalRuntime,
  runtimeDisplayLabel,
  runtimeDisplayName,
  splitRuntimeName,
} from "./display";

describe("runtimeDisplayName", () => {
  it("prefers a custom name when set", () => {
    expect(
      runtimeDisplayName({ name: "Claude (host)", custom_name: "Prod Box" }),
    ).toBe("Prod Box");
  });

  it("trims the custom name", () => {
    expect(
      runtimeDisplayName({ name: "Claude (host)", custom_name: "  Prod Box  " }),
    ).toBe("Prod Box");
  });

  it("falls back to the default name when custom is empty, whitespace, null, or missing", () => {
    expect(runtimeDisplayName({ name: "Claude (host)", custom_name: "" })).toBe(
      "Claude (host)",
    );
    expect(
      runtimeDisplayName({ name: "Claude (host)", custom_name: "   " }),
    ).toBe("Claude (host)");
    expect(
      runtimeDisplayName({ name: "Claude (host)", custom_name: null }),
    ).toBe("Claude (host)");
    expect(runtimeDisplayName({ name: "Claude (host)" })).toBe("Claude (host)");
  });
});

describe("runtimeDisplayLabel", () => {
  it("re-attaches the provider when a custom alias hides it", () => {
    expect(
      runtimeDisplayLabel({
        name: "Codex (EvaM2.local)",
        custom_name: "evam2",
        provider: "codex",
      }),
    ).toBe("evam2 (Codex)");
  });

  it("returns the daemon name unchanged when no alias is set", () => {
    expect(
      runtimeDisplayLabel({
        name: "Codex (EvaM2.local)",
        custom_name: "",
        provider: "codex",
      }),
    ).toBe("Codex (EvaM2.local)");
    expect(
      runtimeDisplayLabel({
        name: "Codex (EvaM2.local)",
        custom_name: null,
        provider: "codex",
      }),
    ).toBe("Codex (EvaM2.local)");
  });

  it("omits the provider suffix when the provider is empty", () => {
    expect(
      runtimeDisplayLabel({ name: "host", custom_name: "evam2", provider: "" }),
    ).toBe("evam2");
  });

  it("uses the daemon's provider display name for overridden slugs", () => {
    // CodeArts, DSH, Qoder CN, Trae, Qwen Code, and QwenPaw use display names that differ from
    // title-cased slugs; aliases must match the daemon's no-alias names.
    expect(
      runtimeDisplayLabel({
        name: "CodeArts (host)",
        custom_name: "box",
        provider: "codearts",
      }),
    ).toBe("box (CodeArts)");
    expect(
      runtimeDisplayLabel({
        name: "DeepSeek Harness (host)",
        custom_name: "box",
        provider: "dsh",
      }),
    ).toBe("box (DeepSeek Harness)");
    expect(
      runtimeDisplayLabel({
        name: "Qoder CN (host)",
        custom_name: "box",
        provider: "qoderclicn",
      }),
    ).toBe("box (Qoder CN)");
    expect(
      runtimeDisplayLabel({
        name: "Trae (host)",
        custom_name: "box",
        provider: "traecli",
      }),
    ).toBe("box (Trae)");
    expect(
      runtimeDisplayLabel({
        name: "Qwen Code (host)",
        custom_name: "box",
        provider: "qwen",
      }),
    ).toBe("box (Qwen Code)");
    expect(
      runtimeDisplayLabel({
        name: "QwenPaw (host)",
        custom_name: "box",
        provider: "qwenpaw",
      }),
    ).toBe("box (QwenPaw)");
    expect(
      runtimeDisplayLabel({
        name: "MiniMax Code (host)",
        custom_name: "box",
        provider: "mcode",
      }),
    ).toBe("box (MiniMax Code)");
    expect(
      runtimeDisplayLabel({
        name: "ZeroClaw (host)",
        custom_name: "box",
        provider: "zeroclaw",
      }),
    ).toBe("box (ZeroClaw)");
  });

  it("first-letter-capitalizes non-overridden slugs, matching the daemon", () => {
    // Providers without an explicit daemon override are first-letter
    // capitalized on both the alias and no-alias paths, so the label must match
    // the daemon (e.g. no-alias name is "Openclaw (host)").
    expect(
      runtimeDisplayLabel({
        name: "Openclaw (host)",
        custom_name: "box",
        provider: "openclaw",
      }),
    ).toBe("box (Openclaw)");
    expect(
      runtimeDisplayLabel({
        name: "Codex (host)",
        custom_name: "box",
        provider: "codex",
      }),
    ).toBe("box (Codex)");
  });
});

describe("device display helpers", () => {
  it("separates a Harness prefix from its machine name", () => {
    expect(splitRuntimeName("Claude (build-server-01)")).toEqual({
      base: "Claude",
      hostname: "build-server-01",
    });
    expect(
      deviceDisplayName({
        name: "Claude (build-server-01)",
        custom_name: null,
        device_info: "build-server-01 · linux-amd64",
        runtime_mode: "local",
        provider: "claude",
      }),
    ).toBe("build-server-01");
  });

  it("treats the current daemon UUID as this machine", () => {
    expect(
      isCurrentLocalRuntime(
        {
          runtime_mode: "local",
          daemon_id: "daemon-new",
          name: "Claude (Mac)",
          device_info: "Mac",
          owner_id: "user-1",
        },
        { localDaemonId: "daemon-new" },
      ),
    ).toBe(true);
  });

  it("still treats this machine as local when the daemon UUID rotated but the host matches", () => {
    expect(
      isCurrentLocalRuntime(
        {
          runtime_mode: "local",
          daemon_id: "daemon-old",
          name: "Claude (Mac)",
          device_info: "Mac · darwin-arm64",
          owner_id: "user-1",
        },
        {
          localDaemonId: "daemon-new",
          localMachineName: "Mac",
          currentUserId: "user-1",
        },
      ),
    ).toBe(true);
  });

  it("labels the viewing user's current computer as this machine", () => {
    expect(
      deviceLabelForViewer(
        {
          runtime_mode: "local",
          daemon_id: "daemon-old",
          name: "Claude (Mac)",
          custom_name: null,
          device_info: "Mac",
          owner_id: "user-1",
          provider: "claude",
        },
        {
          localDaemonId: "daemon-new",
          localMachineName: "Mac",
          currentUserId: "user-1",
        },
        "This machine",
      ),
    ).toBe("This machine");
  });

  it("does not claim another user's identically named host", () => {
    expect(
      isCurrentLocalRuntime(
        {
          runtime_mode: "local",
          daemon_id: "someone-else",
          name: "Claude (Mac)",
          device_info: "Mac",
          owner_id: "user-2",
        },
        {
          localDaemonId: "daemon-new",
          localMachineName: "Mac",
          currentUserId: "user-1",
        },
      ),
    ).toBe(false);
  });

  it("uses a terminal icon family for cloud runtimes", () => {
    expect(
      deviceKind({
        name: "Cloud worker",
        device_info: "",
        runtime_mode: "cloud",
      }),
    ).toBe("terminal");
  });
});
