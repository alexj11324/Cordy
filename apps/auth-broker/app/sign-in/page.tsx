import { redirect } from "next/navigation";
import {
  loginAliasDestination,
  type LoginAliasSearchParams,
} from "@/lib/login-alias";

type SignInAliasPageProps = {
  searchParams: Promise<LoginAliasSearchParams>;
};

/** Keep old Accounts links on the custom shadcn login surface. */
export default async function SignInAliasPage({
  searchParams,
}: SignInAliasPageProps) {
  redirect(loginAliasDestination(await searchParams));
}
