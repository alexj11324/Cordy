const COOKIE_NAME = "patchbay_logged_in";
const LEGACY_COOKIE_NAME = "cordy_logged_in"; // legacy-brand-compat

export function setLoggedInCookie() {
  document.cookie = `${COOKIE_NAME}=1; path=/; max-age=31536000; samesite=lax`;
  document.cookie = `${LEGACY_COOKIE_NAME}=; path=/; max-age=0; samesite=lax`;
}

export function clearLoggedInCookie() {
  document.cookie = `${COOKIE_NAME}=; path=/; max-age=0`;
  document.cookie = `${LEGACY_COOKIE_NAME}=; path=/; max-age=0`;
}
