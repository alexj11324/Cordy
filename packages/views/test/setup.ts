import "@testing-library/jest-dom/vitest";
import { createElement } from "react";
import { vi } from "vitest";

vi.mock("@lobehub/icons", () => {
  const make = (name: string) => {
    function Avatar(props: {
      size?: number;
      shape?: string;
      className?: string;
    }) {
      return createElement("div", {
        "data-lobehub-icon": name,
        "data-size": props.size,
        "data-shape": props.shape ?? "circle",
        className: props.className,
      });
    }
    function Icon() {
      return createElement("span");
    }
    return Object.assign(Icon, { Avatar });
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

function createMemoryStorage(): Storage {
  const values = new Map<string, string>();

  return {
    get length() {
      return values.size;
    },
    clear: () => values.clear(),
    getItem: (key: string) => values.get(key) ?? null,
    key: (index: number) => Array.from(values.keys())[index] ?? null,
    removeItem: (key: string) => {
      values.delete(key);
    },
    setItem: (key: string, value: string) => {
      values.set(key, value);
    },
  };
}

// Everything below patches gaps in jsdom. Pure-logic suites opt out of jsdom
// with `// @vitest-environment node` and share this file, so there is no DOM to
// patch there — bail out rather than guard each stub.
if (typeof window !== "undefined") {
  if (typeof globalThis.localStorage?.clear !== "function") {
    const storage = createMemoryStorage();
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      value: storage,
    });
    Object.defineProperty(window, "localStorage", {
      configurable: true,
      value: storage,
    });
  }

  // jsdom doesn't provide matchMedia; useIsMobile() relies on it.
  if (typeof window.matchMedia !== "function") {
    window.matchMedia = (query: string) =>
      ({
        matches: false,
        media: query,
        onchange: null,
        addListener: () => {},
        removeListener: () => {},
        addEventListener: () => {},
        removeEventListener: () => {},
        dispatchEvent: () => false,
      }) as MediaQueryList;
  }

  // jsdom doesn't provide ResizeObserver; stub it so components that rely on it
  // (e.g. input-otp) can render in tests.
  if (typeof globalThis.ResizeObserver === "undefined") {
    globalThis.ResizeObserver = class ResizeObserver {
      observe() {}
      unobserve() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;
  }

  // jsdom doesn't implement elementFromPoint; input-otp uses it internally.
  if (typeof document.elementFromPoint !== "function") {
    document.elementFromPoint = () => null;
  }

  // jsdom has no layout, so it doesn't implement scrollIntoView; list components
  // that keep a keyboard cursor in view (e.g. the thread navigator) call it.
  if (typeof Element.prototype.scrollIntoView !== "function") {
    Element.prototype.scrollIntoView = () => {};
  }
}
