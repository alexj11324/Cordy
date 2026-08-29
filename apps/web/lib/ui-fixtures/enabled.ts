type Env = Record<string, string | undefined>;

/** True when `make web-dev` (or `PATCHBAY_UI_FIXTURES=1`) is serving product UI without Rust. */
export function isUiFixturesEnabled(env: Env = process.env): boolean {
  return env.NODE_ENV !== "production" && env.PATCHBAY_UI_FIXTURES === "1";
}
