export function buildDesktopGoogleLoginUrl(accountsUrl: string): string {
  const url = new URL(accountsUrl);
  url.pathname = `${url.pathname.replace(/\/+$/, "")}/oauth/google`;
  url.search = "";
  url.hash = "";
  url.searchParams.set("platform", "desktop");
  return url.href;
}
