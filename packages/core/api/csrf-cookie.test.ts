// @vitest-environment node

import { describe, expect, it } from "vitest";
import {
  PRODUCTION_CSRF_COOKIE_NAME,
  STAGING_CSRF_COOKIE_NAME,
  readCsrfTokenFromCookieHeader,
} from "./csrf-cookie";

describe("readCsrfTokenFromCookieHeader", () => {
  it("returns the production CSRF cookie when it is the only match", () => {
    expect(
      readCsrfTokenFromCookieHeader(`${PRODUCTION_CSRF_COOKIE_NAME}=prod-token; other=1`),
    ).toBe("prod-token");
  });

  it("prefers the staging CSRF cookie when both names are present", () => {
    expect(
      readCsrfTokenFromCookieHeader(
        `${PRODUCTION_CSRF_COOKIE_NAME}=prod-token; ${STAGING_CSRF_COOKIE_NAME}=staging-token`,
      ),
    ).toBe("staging-token");
    expect(
      readCsrfTokenFromCookieHeader(
        `${STAGING_CSRF_COOKIE_NAME}=staging-token; ${PRODUCTION_CSRF_COOKIE_NAME}=prod-token`,
      ),
    ).toBe("staging-token");
  });

  it("ignores a prefix collision with the production name", () => {
    expect(readCsrfTokenFromCookieHeader("orvilo_csrf_extra=nope")).toBeNull();
  });

  it("returns null for an empty header", () => {
    expect(readCsrfTokenFromCookieHeader("")).toBeNull();
    expect(readCsrfTokenFromCookieHeader("   ")).toBeNull();
  });
});
