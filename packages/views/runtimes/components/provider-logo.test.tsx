import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ProviderLogo } from "./provider-logo";

const coordyProviders = [
  "claude",
  "codebuddy",
  "codex",
  "copilot",
  "opencode",
  "deveco",
  "openclaw",
  "hermes",
  "pi",
  "omp",
  "cursor",
  "kimi",
  "reasonix",
  "dsh",
  "kiro",
  "antigravity",
  "qoder",
  "qoderclicn",
  "traecli",
  "grok",
  "gemini",
  "qwen",
  "qwenpaw",
  "mcode",
] as const;

describe("ProviderLogo", () => {
  it.each(coordyProviders)("renders the local Coordy %s asset", (provider) => {
    const { container } = render(
      <ProviderLogo provider={provider} className="runtime-logo" />,
    );

    const logo = container.querySelector("img");

    expect(logo?.getAttribute("src")).toBeTruthy();
    expect(logo?.getAttribute("alt")).toBe("");
    expect(logo?.getAttribute("aria-hidden")).toBe("true");
    expect(logo?.classList.contains("runtime-logo")).toBe(true);
  });

  it("uses the dedicated Grok asset instead of the Reasonix artwork", () => {
    const grok = render(<ProviderLogo provider="grok" />).container.querySelector("img");
    const reasonix = render(<ProviderLogo provider="reasonix" />).container.querySelector("img");

    expect(grok?.getAttribute("src")).toBeTruthy();
    expect(reasonix?.getAttribute("src")).toBeTruthy();
    expect(grok?.getAttribute("src")).not.toBe(reasonix?.getAttribute("src"));
  });

  it.each([
    ["codearts", "CodeArts"],
    ["dim", ""],
  ] as const)("keeps the existing %s compatibility asset", (provider, alt) => {
    const { container } = render(
      <ProviderLogo provider={provider} className="runtime-logo" />,
    );

    const logo = container.querySelector("img");
    expect(logo?.getAttribute("alt")).toBe(alt);
    expect(logo?.classList.contains("runtime-logo")).toBe(true);
  });

  it("keeps the existing ZeroClaw compatibility mark", () => {
    const { container } = render(
      <ProviderLogo provider="zeroclaw" className="runtime-logo" />,
    );

    const logo = container.querySelector("svg");
    expect(logo?.getAttribute("viewBox")).toBe("0 0 24 24");
    expect(logo?.querySelectorAll("path")).toHaveLength(3);
    expect(logo?.classList.contains("runtime-logo")).toBe(true);
  });

  it("keeps the generic fallback for providers absent from Coordy", () => {
    const { container } = render(
      <ProviderLogo provider="custom-provider" className="runtime-logo" />,
    );

    const logo = container.querySelector("svg.lucide-monitor");
    expect(logo?.classList.contains("runtime-logo")).toBe(true);
  });
});
