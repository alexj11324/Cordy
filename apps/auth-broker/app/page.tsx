import { redirect } from "next/navigation";
import { AUTH_CONTRACT } from "@/lib/contract";

export default function HomePage(): never {
  redirect(`${AUTH_CONTRACT.origins.product}/login`);
}
