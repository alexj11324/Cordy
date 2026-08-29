"use client";

import { UI_FIXTURE_COOKIE } from "@/lib/ui-fixtures/mode";

function open(mode: "onboarding" | "app", href: string) {
  document.cookie = `${UI_FIXTURE_COOKIE}=${mode}; path=/; samesite=lax`;
  if (mode === "app") {
    document.cookie = "last_workspace_slug=preview; path=/; samesite=lax";
  }
  window.location.assign(href);
}

export default function UiPreviewIndexPage() {
  return (
    <main className="mx-auto flex min-h-dvh max-w-lg flex-col justify-center gap-6 p-8">
      <h1 className="text-title-lg font-semibold">Product UI</h1>
      <p className="text-body text-muted-foreground">
        These are the real onboarding and app routes. The local fixture API
        stands in for Rust so `make web-dev` can render them without a backend.
      </p>
      <button
        type="button"
        className="text-left text-body underline"
        onClick={() => open("onboarding", "/onboarding")}
      >
        Onboarding
      </button>
      <button
        type="button"
        className="text-left text-body underline"
        onClick={() => open("app", "/preview/issues")}
      >
        App / Issues
      </button>
    </main>
  );
}
