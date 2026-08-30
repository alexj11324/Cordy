export function buildDesktopGoogleLoginUrl(appUrl: string): string {
  const url = new URL("/oauth/google", appUrl);
  if (url.hostname === "accounts.aspectlylabs.com") {
    throw new Error("Legacy accounts login origin is not supported");
  }
  url.searchParams.set("platform", "desktop");
  return url.href;
}
