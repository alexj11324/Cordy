import { act, render } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { coreProps, resetWelcome, clearLoggedInCookie, signOut } = vi.hoisted(
  () => ({
    coreProps: {
      current: null as null | { onLogout?: () => void | Promise<void> },
    },
    resetWelcome: vi.fn(),
    clearLoggedInCookie: vi.fn(),
    signOut: vi.fn(),
  }),
);

vi.mock("@clerk/nextjs", () => ({
  useAuth: () => ({ isSignedIn: true, signOut }),
}));

vi.mock("@patchbay/core/platform", () => ({
  CoreProvider: (props: {
    children: ReactNode;
    onLogout?: () => void | Promise<void>;
  }) => {
    coreProps.current = props;
    return props.children;
  },
}));

vi.mock("@patchbay/core/i18n/browser", () => ({
  createBrowserCookieLocaleAdapter: () => ({}),
}));

vi.mock("@patchbay/core/onboarding", () => ({
  useWelcomeStore: {
    getState: () => ({ reset: resetWelcome }),
  },
}));

vi.mock("@/platform/navigation", () => ({
  WebNavigationProvider: ({ children }: { children: ReactNode }) => children,
}));

vi.mock("@/platform/scroll-restoration", () => ({
  WebScrollRestorationProvider: ({ children }: { children: ReactNode }) =>
    children,
}));

vi.mock("@/features/auth/auth-cookie", () => ({
  setLoggedInCookie: vi.fn(),
  clearLoggedInCookie,
}));

vi.mock("@/platform/client-os", () => ({
  detectWebOS: () => "macos",
}));

vi.mock("./clerk-auth-adapter", () => ({
  ClerkAuthAdapter: ({ children }: { children: ReactNode }) => children,
}));

vi.mock("@/lib/ui-fixtures/context", () => ({
  UiFixturesProvider: ({ children }: { children: ReactNode }) => children,
}));

import { WebProviders } from "./web-providers";

describe("WebProviders", () => {
  beforeEach(() => {
    coreProps.current = null;
    vi.clearAllMocks();
    signOut.mockResolvedValue(undefined);
  });

  it("revokes the Clerk session before completing normal web logout", async () => {
    render(
      <WebProviders locale="en" resources={{}}>
        <div>content</div>
      </WebProviders>,
    );

    const onLogout = coreProps.current?.onLogout;
    expect(onLogout).toBeTypeOf("function");
    await act(async () => onLogout?.());

    expect(signOut).toHaveBeenCalledOnce();
    expect(resetWelcome).toHaveBeenCalledOnce();
    expect(clearLoggedInCookie).toHaveBeenCalledOnce();
    expect(signOut.mock.invocationCallOrder[0]!).toBeLessThan(
      clearLoggedInCookie.mock.invocationCallOrder[0]!,
    );
  });

  it("completes local logout when Clerk sign-out is temporarily unavailable", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    signOut.mockRejectedValue(new TypeError("offline"));
    render(
      <WebProviders locale="en" resources={{}}>
        <div>content</div>
      </WebProviders>,
    );

    const onLogout = coreProps.current?.onLogout;
    await expect(act(async () => onLogout?.())).resolves.toBeUndefined();

    expect(signOut).toHaveBeenCalledOnce();
    expect(resetWelcome).toHaveBeenCalledOnce();
    expect(clearLoggedInCookie).toHaveBeenCalledOnce();
    warn.mockRestore();
  });
});
