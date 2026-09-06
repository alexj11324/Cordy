import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

const clerkProps = vi.hoisted(() => ({ current: null as Record<string, unknown> | null }));

vi.mock("@clerk/nextjs", () => ({
  ClerkProvider: (props: Record<string, unknown>) => {
    clerkProps.current = props;
    return <>{props.children}</>;
  },
}));


import { ClerkProvider } from "./clerk-provider";

describe("ClerkProvider", () => {
  it("passes the runtime publishable key into the browser provider", () => {
    render(
      <ClerkProvider publishableKey="pk_live_runtime-key">
        <div data-testid="content" />
      </ClerkProvider>,
    );

    expect(screen.getByTestId("content")).toBeInTheDocument();
    expect(clerkProps.current).toMatchObject({
      publishableKey: "pk_live_runtime-key",
    });
  });
});
