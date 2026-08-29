export const dynamic = "force-static";

export function GET() {
  return Response.json({ service: "docs", status: "ok" });
}
