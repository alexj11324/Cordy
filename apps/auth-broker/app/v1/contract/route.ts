import { AUTH_CONTRACT, authContractResponseHeaders } from "@/lib/contract";

export function GET(): Response {
  return Response.json(AUTH_CONTRACT, {
    headers: authContractResponseHeaders(),
  });
}
