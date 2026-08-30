export function resolveSafeRedirectUrl(raw: string | null): string {
  if (!raw) return "/";

  if (raw.startsWith("/") && !raw.startsWith("//")) {
    const url = new URL(raw, "https://patchbay.invalid");
    return `${url.pathname}${url.search}${url.hash}` || "/";
  }

  if (typeof window === "undefined") return "/";
  try {
    const url = new URL(raw);
    if (url.origin !== window.location.origin) return "/";
    return `${url.pathname}${url.search}${url.hash}` || "/";
  } catch {
    return "/";
  }
}

export function authRouteWithRedirect(
  route: "/login" | "/signup",
  redirectUrl: string,
): string {
  if (redirectUrl === "/") return route;
  return `${route}?${new URLSearchParams({ redirect_url: redirectUrl })}`;
}
