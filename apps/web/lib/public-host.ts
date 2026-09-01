const OFFICIAL_MARKETING_HOSTS = new Set(["patchbay.ai", "www.patchbay.ai"]);

export function isOfficialMarketingHost(hostname: string): boolean {
  const normalized = hostname.trim().toLowerCase().replace(/\.$/, "");
  return OFFICIAL_MARKETING_HOSTS.has(normalized);
}
