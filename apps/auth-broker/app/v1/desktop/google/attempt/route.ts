import { readAuthBrokerRuntimeConfig } from "@/lib/runtime-config";
import { proxyRustDesktopGoogleRequest } from "@/lib/rust-api-proxy";
import { authContractResponseHeaders } from "@/lib/contract";

export async function POST(request: Request): Promise<Response> {
  const runtime = readAuthBrokerRuntimeConfig();
  if (!runtime.ok) {
    return Response.json(
      { error: "broker_not_ready" },
      { status: 503, headers: authContractResponseHeaders() },
    );
  }
  return proxyRustDesktopGoogleRequest(request, "attempt", runtime.config);
}
