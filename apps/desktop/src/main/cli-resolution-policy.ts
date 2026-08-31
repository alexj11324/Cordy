export function requiresSourceMatchedCli(
  env: NodeJS.ProcessEnv = process.env,
): boolean {
  return env.PATCHBAY_REQUIRE_SOURCE_CLI === "1";
}
