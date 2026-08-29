export const dynamic = "force-static";

export function GET() {
  return Response.json({ service: "web", status: "ok" });
}
