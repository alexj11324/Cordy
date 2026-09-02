import { readAuthBrokerRuntimeConfig } from "@/lib/runtime-config";
export function GET(): Response { const config = readAuthBrokerRuntimeConfig(); return Response.json({ status: config.ok ? "ok" : "not_ready" }, { status: config.ok ? 200 : 503, headers: { "cache-control": "no-store" } }); }
