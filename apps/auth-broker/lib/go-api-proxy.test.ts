import { describe, expect, it, vi } from "vitest";
import { proxyGoDesktopGoogleRequest } from "./go-api-proxy";
const config = { apiOrigin: "https://api.aspectlylabs.com", brokerOrigin: "https://accounts.aspectlylabs.com", goBrokerAuthToken: "a".repeat(64) };
function request(headers: Record<string, string> = {}) { return new Request("https://accounts.aspectlylabs.com/v1/desktop/google/attempt", { method: "POST", headers: { origin: config.brokerOrigin, "content-type": "application/json", "x-patchbay-auth-contract-version": "1", ...headers }, body: JSON.stringify({ state: "s".repeat(43), code_challenge: "c".repeat(43) }) }); }
describe("Go API proxy", () => {
  it("sends only constructed auth headers to the Go origin", async () => { const fetcher = vi.fn().mockResolvedValue(Response.json({ registered: true })); await expect(proxyGoDesktopGoogleRequest(request({ cookie: "secret", "x-forwarded-host": "evil" }), "attempt", config, fetcher)).resolves.toHaveProperty("status", 200); const init = fetcher.mock.calls[0]?.[1] as RequestInit; const headers = new Headers(init.headers); expect(headers.get("cookie")).toBeNull(); expect(headers.get("x-forwarded-host")).toBeNull(); expect(headers.get("x-patchbay-desktop-broker-auth")).toBe(config.goBrokerAuthToken); });
  it("rejects cross-origin calls", async () => { const response = await proxyGoDesktopGoogleRequest(request({ origin: "https://evil.example" }), "attempt", config); expect(response.status).toBe(403); });
});
