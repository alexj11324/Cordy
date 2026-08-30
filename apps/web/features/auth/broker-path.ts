const GOOGLE_OAUTH_ENTRY_PATH = "/oauth/google";
const GOOGLE_OAUTH_CALLBACK_PATH = "/oauth/google/callback";

type BrokerRoute =
  | "/login"
  | typeof GOOGLE_OAUTH_ENTRY_PATH
  | typeof GOOGLE_OAUTH_CALLBACK_PATH;

/** Keep a reverse-proxy base path on every same-origin broker transition. */
export function buildBrokerRoute(
  currentPathname: string,
  currentRoute: BrokerRoute,
  targetRoute: BrokerRoute,
): string {
  const pathname = currentPathname.replace(/\/+$/, "");
  if (
    !pathname.startsWith("/") ||
    pathname.startsWith("//") ||
    pathname.includes("\\") ||
    !pathname.endsWith(currentRoute)
  ) {
    return targetRoute;
  }

  const basePath = pathname.slice(0, -currentRoute.length);
  if (basePath.includes("//")) return targetRoute;
  return `${basePath}${targetRoute}`;
}
