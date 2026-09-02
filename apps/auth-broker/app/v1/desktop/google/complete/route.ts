import { readAuthBrokerRuntimeConfig } from "@/lib/runtime-config";
import { proxyGoDesktopGoogleRequest } from "@/lib/go-api-proxy";
import { authContractResponseHeaders } from "@/lib/contract";
export async function POST(request: Request): Promise<Response> { const runtime = readAuthBrokerRuntimeConfig(); if (!runtime.ok) return Response.json({ error: "broker_not_ready" }, { status: 503, headers: authContractResponseHeaders() }); return proxyGoDesktopGoogleRequest(request, "complete", runtime.config); }
