export const UI_FIXTURE_COOKIE = "patchbay_ui_fixture";

export type UiFixtureMode = "onboarding" | "app";

export function parseUiFixtureMode(
  cookieHeader: string | null | undefined,
): UiFixtureMode {
  const match = cookieHeader?.match(
    new RegExp(`(?:^|;\\s*)${UI_FIXTURE_COOKIE}=([^;]*)`),
  );
  return match?.[1] === "onboarding" ? "onboarding" : "app";
}

export function uiFixtureModeCookie(mode: UiFixtureMode): string {
  return `${UI_FIXTURE_COOKIE}=${mode}; path=/; samesite=lax`;
}
