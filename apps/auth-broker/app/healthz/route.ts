import {
  AUTH_CONTRACT_VERSION,
  authContractResponseHeaders,
} from "@/lib/contract";

export function GET(): Response {
  return Response.json(
    { status: "ok", contract_version: AUTH_CONTRACT_VERSION },
    { headers: authContractResponseHeaders() },
  );
}
