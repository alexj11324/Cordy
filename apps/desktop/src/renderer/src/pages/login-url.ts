function normalizeBrowserReturnOrigin(value: string): string {
  const url = new URL(value);
  if (
    (url.protocol !== "http:" && url.protocol !== "https:") ||
    url.username ||
    url.password ||
    url.pathname !== "/" ||
    url.search ||
    url.hash
  ) {
    throw new Error("Desktop browser return origin must be an HTTP(S) origin");
  }
  return url.origin;
}

export function buildDesktopGoogleLoginUrl(
  appUrl: string,
  browserReturnOrigin?: string,
): string {
  const url = new URL("/oauth/google", appUrl);
  if (url.hostname === "accounts.aspectlylabs.com") {
    throw new Error("Legacy accounts login origin is not supported");
  }
  url.searchParams.set("platform", "desktop");
  if (browserReturnOrigin) {
    url.searchParams.set(
      "app_origin",
      normalizeBrowserReturnOrigin(browserReturnOrigin),
    );
  }
  return url.href;
}
