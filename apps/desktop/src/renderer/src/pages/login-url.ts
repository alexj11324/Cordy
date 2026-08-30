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
  accountsUrl: string,
  browserReturnOrigin?: string,
): string {
  const url = new URL("/oauth/google", accountsUrl);
  url.searchParams.set("platform", "desktop");
  if (browserReturnOrigin) {
    url.searchParams.set(
      "app_origin",
      normalizeBrowserReturnOrigin(browserReturnOrigin),
    );
  }
  return url.href;
}
