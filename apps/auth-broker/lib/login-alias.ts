export type LoginAliasSearchParams = Record<
  string,
  string | string[] | undefined
>;

/** Preserve auth and return parameters while canonicalizing /sign-in to /login. */
export function loginAliasDestination(
  searchParams: LoginAliasSearchParams,
): string {
  const query = new URLSearchParams();
  for (const [key, value] of Object.entries(searchParams)) {
    if (Array.isArray(value)) {
      for (const item of value) query.append(key, item);
    } else if (typeof value === "string") {
      query.set(key, value);
    }
  }
  const serialized = query.toString();
  return serialized ? `/login?${serialized}` : "/login";
}
