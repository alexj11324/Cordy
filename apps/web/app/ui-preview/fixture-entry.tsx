"use client";

import { useLayoutEffect } from "react";
import {
  UI_FIXTURE_COOKIE,
  type UiFixtureMode,
} from "@/lib/ui-fixtures/mode";

export function FixtureEntry({
  mode,
  href,
}: {
  mode: UiFixtureMode;
  href: string;
}) {
  useLayoutEffect(() => {
    document.cookie = `${UI_FIXTURE_COOKIE}=${mode}; path=/; samesite=lax`;
    if (mode === "app") {
      document.cookie = "last_workspace_slug=preview; path=/; samesite=lax";
    }
    window.location.replace(href);
  }, [href, mode]);
  return null;
}
