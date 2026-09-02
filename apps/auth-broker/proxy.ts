import { NextRequest, NextResponse } from "next/server";

const ORIGIN_AUTH_HEADER = "x-patchbay-origin-auth";

export function proxy(request: NextRequest) {
  if (request.nextUrl.pathname === "/healthz" || request.nextUrl.pathname === "/readyz") return NextResponse.next();
  const expected = process.env.PATCHBAY_ORIGIN_AUTH_TOKEN?.trim() ?? "";
  const supplied = request.headers.get(ORIGIN_AUTH_HEADER) ?? "";
  const valid = /^[a-f0-9]{64}$/.test(expected) && constantTimeEqual(supplied, expected);
  if (!valid) return new NextResponse("Not Found\n", { status: 404, headers: { "cache-control": "no-store", "content-type": "text/plain; charset=utf-8" } });
  const headers = new Headers(request.headers);
  headers.delete(ORIGIN_AUTH_HEADER);
  headers.delete("x-patchbay-desktop-broker-auth");
  return NextResponse.next({ request: { headers } });
}

function constantTimeEqual(left: string, right: string): boolean {
  if (left.length !== right.length) return false;
  let difference = 0;
  for (let index = 0; index < left.length; index += 1) difference |= left.charCodeAt(index) ^ right.charCodeAt(index);
  return difference === 0;
}

export const config = { matcher: ["/((?!_next/static|_next/image|favicon.ico).*)"] };
