import "@testing-library/jest-dom/vitest";

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

  // useIsMobile() and antd-style's useResponsive() rely on matchMedia.
  //
  // **This guard fires, and this stub is live.** jsdom does not implement
  // `matchMedia` — verified directly against the version this repo resolves:
  //
  //   $ node -e "const {JSDOM}=require('jsdom');
  //              console.log(typeof new JSDOM('').window.matchMedia)"
  //   undefined                     // jsdom 29.0.1
  //
  // and inside the vitest environment the function is this arrow function, not
  // jsdom's: `String(window.matchMedia)` starts `(query) => ({` and the object
  // it returns has `Object` for its prototype rather than `MediaQueryList`.
  //
  // So deleting the stub is not a tidy-up: every suite that mounts a Lobe
  // component would fail with `TypeError: window.matchMedia is not a function`,
  // because `useResponsive()` and `useIsMobile()` call it during render.
  // Observed, by making the guard above not fire: 13 of the preferences suite's
  // tests fail immediately with exactly that TypeError, plus a downstream
  // `Cannot read properties of undefined (reading 'addEventListener')` from the
  // callers that receive `undefined` back. It is
  // also why `useResponsive().mobile` is false in tests and why
  // `prefers-reduced-motion` always answers false — the stub answers `false` to
  // every query, which is what antd-style needs to pick its desktop defaults
  // under jsdom.
  //
  // A note for whoever reads this next: an earlier revision of this comment
  // claimed the opposite — that jsdom ships its own `matchMedia` and the stub
  // was dead code — on the strength of a second-hand, self-labelled
  // "not independently verified" claim. It was wrong, and it actively invited
  // someone to delete a load-bearing stub. Hence the command above: the claim
  // is cheap to re-check and expensive to trust.
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
