const VALUE = /^[A-Za-z0-9._~-]{43,128}$/;
const CODE = /^pb[dl]_[A-Za-z0-9_-]{43}$/;
const PROTOCOL =
  /^(?:patchbay|patchbay-canary(?:-[a-z0-9](?:[a-z0-9-]{0,46}[a-z0-9])?)?)$/;

export type DesktopBinding = {
  state: string;
  codeChallenge: string;
  query: string;
  local: boolean;
};

export function readDesktopHandoffBinding(
  params: URLSearchParams,
): DesktopBinding | null {
  if (params.get("platform") !== "desktop" || params.has("app_origin") || params.has("session_api")) {
    return null;
  }
  const state = params.get("state") ?? "";
  const codeChallenge = params.get("code_challenge") ?? "";
  if (!VALUE.test(state) || !VALUE.test(codeChallenge)) return null;
  const mode = params.get("session_mode");
  if (mode !== null && mode !== "local") return null;
  const local = mode === "local";
  const query = new URLSearchParams({
    platform: "desktop",
    state,
    code_challenge: codeChallenge,
  });
  if (local) query.set("session_mode", "local");
  return { state, codeChallenge, query: query.toString(), local };
}

export function isDesktopHandoffInput(
  value: unknown,
): value is { state: string; code_challenge: string; local?: boolean } {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const input = value as Record<string, unknown>;
  return (
    typeof input.state === "string" &&
    typeof input.code_challenge === "string" &&
    VALUE.test(input.state) &&
    VALUE.test(input.code_challenge) &&
    (input.local === undefined || typeof input.local === "boolean")
  );
}

export function buildDesktopCallbackUrl(
  code: string,
  state: string,
  protocol: string,
): string {
  if (!isDesktopCode(code) || !VALUE.test(state) || !isDesktopCallbackProtocol(protocol)) {
    throw new Error("invalid desktop callback");
  }
  const url = new URL(`${protocol}://auth/callback`);
  url.searchParams.set("code", code);
  url.searchParams.set("state", state);
  return url.href;
}

export const isDesktopCode = (value: unknown): value is string =>
  typeof value === "string" && CODE.test(value);

export const isDesktopCallbackProtocol = (value: unknown): value is string =>
  typeof value === "string" && PROTOCOL.test(value);
