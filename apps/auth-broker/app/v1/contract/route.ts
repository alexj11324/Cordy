import { AUTH_CONTRACT } from "@/lib/contract";
export function GET(): Response { return Response.json(AUTH_CONTRACT, { headers: { "cache-control": "no-store" } }); }
