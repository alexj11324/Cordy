import {
  AUTH_CONTRACT_HEADER,
  AUTH_CONTRACT_VERSION,
  DESKTOP_ATTEMPT_PATH,
  DESKTOP_COMPLETE_PATH,
} from "./contract";
import { isDesktopCode } from "./desktop-handoff";

export class BrokerApiError extends Error {
  constructor(public readonly status: number) {
    super(`Auth broker request failed (${status})`);
  }
}

type DesktopBindingInput = { state: string; code_challenge: string };

export async function registerDesktopGoogleAttempt(
  input: DesktopBindingInput,
): Promise<void> {
  const response = await post(DESKTOP_ATTEMPT_PATH, input);
  const payload: unknown = await response.json();
  if (
    !payload ||
    typeof payload !== "object" ||
    (payload as Record<string, unknown>).registered !== true
  ) {
    throw new BrokerApiError(502);
  }
}

export async function completeDesktopGoogleAttempt(
  sessionToken: string,
  input: DesktopBindingInput,
): Promise<string> {
  const response = await post(DESKTOP_COMPLETE_PATH, input, sessionToken);
  const payload: unknown = await response.json();
  const code =
    payload && typeof payload === "object"
      ? (payload as Record<string, unknown>).code
      : null;
  if (!isDesktopCode(code)) throw new BrokerApiError(502);
  return code;
}

async function post(
  path: string,
  body: DesktopBindingInput,
  sessionToken?: string,
): Promise<Response> {
  const headers = new Headers({
    "content-type": "application/json",
    [AUTH_CONTRACT_HEADER]: String(AUTH_CONTRACT_VERSION),
  });
  if (sessionToken) headers.set("authorization", `Bearer ${sessionToken}`);
  const response = await fetch(path, {
    method: "POST",
    headers,
    body: JSON.stringify(body),
    credentials: "same-origin",
    cache: "no-store",
    redirect: "error",
  });
  if (!response.ok) throw new BrokerApiError(response.status);
  return response;
}
