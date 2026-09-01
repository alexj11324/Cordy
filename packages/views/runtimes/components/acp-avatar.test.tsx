import { describe, expect, it, vi } from "vitest";
import { render } from "@testing-library/react";
import { AcpAvatar } from "./acp-avatar";
import { ALL_RUNTIME_PROVIDERS } from "./acp-avatar-map";

vi.mock("@lobehub/icons", () => {
  const make = (name: string) => {
    function Icon() {
      return null;
    }
    Icon.Avatar = ({
      size,
      shape,
    }: {
      size: number;
      shape?: string;
    }) => (
      <div
        data-lobehub-icon={name}
        data-size={size}
        data-shape={shape ?? "circle"}
      />
    );
    return Icon;
  };
  return {
    Antigravity: make("Antigravity"),
    ClaudeCode: make("ClaudeCode"),
    CodeBuddy: make("CodeBuddy"),
    Codex: make("Codex"),
    Copilot: make("Copilot"),
    Cursor: make("Cursor"),
    DeepSeek: make("DeepSeek"),
    Grok: make("Grok"),
    HermesAgent: make("HermesAgent"),
    Huawei: make("Huawei"),
    Kimi: make("Kimi"),
    Kiro: make("Kiro"),
    Minimax: make("Minimax"),
    OpenClaw: make("OpenClaw"),
    OpenCode: make("OpenCode"),
    Pi: make("Pi"),
    Qoder: make("Qoder"),
    Qwen: make("Qwen"),
    Trae: make("Trae"),
  };
});

describe("AcpAvatar", () => {
  it("renders a lobehub avatar for every provider that has one", () => {
    const lobehubProviders = ALL_RUNTIME_PROVIDERS.filter(
      (provider) => provider !== "reasonix" && provider !== "dim",
    );
    for (const provider of lobehubProviders) {
      const { getByTestId, unmount } = render(
        <AcpAvatar provider={provider} size={32} name={provider} />,
      );
      const root = getByTestId("acp-avatar");
      expect(root).toHaveAttribute("data-source", "lobehub");
      expect(root).toHaveAttribute("data-provider", provider);
      expect(root.querySelector("[data-lobehub-icon]")).not.toBeNull();
      unmount();
    }
  });

  it("uses a square 64px face for the Agent welcome avatar", () => {
    const { getByTestId } = render(
      <AcpAvatar provider="kiro" size={64} shape="square" name="Kiro" />,
    );
    const icon = getByTestId("acp-avatar").querySelector("[data-lobehub-icon]");
    expect(icon).toHaveAttribute("data-lobehub-icon", "Kiro");
    expect(icon).toHaveAttribute("data-size", "64");
    expect(icon).toHaveAttribute("data-shape", "square");
  });

  it("falls back to ProviderLogo for dim and reasonix", () => {
    for (const provider of ["dim", "reasonix"] as const) {
      const { getByTestId, unmount } = render(
        <AcpAvatar provider={provider} size={32} name={provider} />,
      );
      expect(getByTestId("acp-avatar")).toHaveAttribute("data-source", "fallback");
      unmount();
    }
  });
});
