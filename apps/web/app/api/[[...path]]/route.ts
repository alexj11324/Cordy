import { NextResponse, type NextRequest } from "next/server";
import { resolveDevRemoteApiUrl } from "@/config/runtime-urls";
import { isUiFixturesEnabled } from "@/lib/ui-fixtures/enabled";
import { handleFixtureRequest } from "@/lib/ui-fixtures/handler";

export const dynamic = "force-dynamic";
export const runtime = "nodejs";

type RouteContext = { params: Promise<{ path?: string[] }> };

async function readBody(req: NextRequest): Promise<unknown> {
  if (req.method === "GET" || req.method === "HEAD") return undefined;
  const text = await req.text();
  if (!text) return undefined;
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return text;
  }
}

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

async function handle(req: NextRequest, context: RouteContext): Promise<Response> {
  if (!isUiFixturesEnabled()) {
    if (process.env.NODE_ENV === "production") {
      return NextResponse.json({ error: "Not found" }, { status: 404 });
    }
    return proxyToBackend(req);
  }

  const { path = [] } = await context.params;
  const result = handleFixtureRequest({
    method: req.method,
    pathname: `/api/${path.join("/")}`,
    search: req.nextUrl.searchParams,
    cookieHeader: req.headers.get("cookie"),
    workspaceSlug: req.headers.get("x-workspace-slug"),
    body: await readBody(req),
  });

  if (result.status === 204) {
    return new NextResponse(null, { status: 204 });
  }
  return NextResponse.json(result.body ?? null, { status: result.status });
}

export const GET = handle;
export const POST = handle;
export const PATCH = handle;
export const PUT = handle;
export const DELETE = handle;
