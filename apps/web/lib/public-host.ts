const OFFICIAL_MARKETING_HOSTS = new Set(["patchbay.aspectlylabs.com"]);

export function isOfficialMarketingHost(hostname: string): boolean {
  const normalized = hostname.trim().toLowerCase().replace(/\.$/, "");
  return OFFICIAL_MARKETING_HOSTS.has(normalized);
}
