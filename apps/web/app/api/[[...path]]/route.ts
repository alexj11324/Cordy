import { NextResponse, type NextRequest } from "next/server";
import { resolveDevRemoteApiUrl } from "@/config/runtime-urls";

export const dynamic = "force-dynamic";
export const runtime = "nodejs";

async function proxyToBackend(req: NextRequest): Promise<Response> {
  const origin = resolveDevRemoteApiUrl(process.env);
  const incoming = new URL(req.url);
  const headers = new Headers(req.headers);
  headers.delete("host");
  headers.delete("connection");
  const init: RequestInit = {
    method: req.method,
    headers,
    redirect: "manual",
  };
  if (req.method !== "GET" && req.method !== "HEAD") {
    init.body = req.body;
    Object.assign(init, { duplex: "half" });
  }
  try {
    const upstream = await fetch(`${origin}${incoming.pathname}${incoming.search}`, init);
    return new NextResponse(upstream.body, {
      status: upstream.status,
      headers: upstream.headers,
    });
  } catch {
    return NextResponse.json(
      { error: "Backend unavailable" },
      { status: 502 },
    );
  }
}

async function handle(req: NextRequest): Promise<Response> {
  if (process.env.NODE_ENV === "production") {
    return NextResponse.json({ error: "Not found" }, { status: 404 });
  }
  return proxyToBackend(req);
}

export const GET = handle;
export const POST = handle;
export const PATCH = handle;
export const PUT = handle;
export const DELETE = handle;
