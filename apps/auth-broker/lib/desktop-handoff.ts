import { DESKTOP_CALLBACK_URL } from "./contract";

const HANDOFF_VALUE_PATTERN = /^[A-Za-z0-9._~-]{43,128}$/;
const DESKTOP_CODE_PATTERN = /^pbd_[A-Za-z0-9_-]{43,252}$/;

export type DesktopHandoffBinding = {
  codeChallenge: string;
  state: string;
  query: string;
};

export function readDesktopHandoffBinding(
  searchParams: URLSearchParams,
): DesktopHandoffBinding | null {
  if (searchParams.get("platform") !== "desktop") return null;
  if (searchParams.has("app_origin")) return null;
  const codeChallenge = searchParams.get("code_challenge") ?? "";
  const state = searchParams.get("state") ?? "";
  if (
    !HANDOFF_VALUE_PATTERN.test(codeChallenge) ||
    !HANDOFF_VALUE_PATTERN.test(state)
  ) {
    return null;
  }
  const query = new URLSearchParams({
    platform: "desktop",
    code_challenge: codeChallenge,
    state,
  }).toString();
  return { codeChallenge, state, query };
}

export function isDesktopHandoffInput(
  value: unknown,
): value is { code_challenge: string; state: string } {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const input = value as Record<string, unknown>;
  return (
    typeof input.code_challenge === "string" &&
    typeof input.state === "string" &&
    HANDOFF_VALUE_PATTERN.test(input.code_challenge) &&
    HANDOFF_VALUE_PATTERN.test(input.state)
  );
}

export function buildDesktopCallbackUrl(code: string, state: string): string {
  if (!DESKTOP_CODE_PATTERN.test(code) || !HANDOFF_VALUE_PATTERN.test(state)) {
    throw new Error("invalid desktop callback");
  }
  const url = new URL(DESKTOP_CALLBACK_URL);
  url.searchParams.set("code", code);
  url.searchParams.set("state", state);
  return url.href;
}

export function isDesktopCode(value: unknown): value is string {
  return typeof value === "string" && DESKTOP_CODE_PATTERN.test(value);
}
