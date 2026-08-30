import { authContractResponseHeaders } from "@/lib/contract";
import { readAuthBrokerRuntimeConfig } from "@/lib/runtime-config";

export function GET(): Response {
  const runtime = readAuthBrokerRuntimeConfig();
  return Response.json(
    { status: runtime.ok ? "ready" : "not_ready" },
    {
      status: runtime.ok ? 200 : 503,
      headers: authContractResponseHeaders(),
    },
  );
}
