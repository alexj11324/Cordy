import { clerkMiddleware, createRouteMatcher } from "@clerk/nextjs/server";
import {
  NextResponse,
  type NextFetchEvent,
  type NextRequest,
} from "next/server";
import { LOCALE_COOKIE } from "@patchbay/core/i18n";
import {
  PATCHBAY_LOCALE_HEADER,
  resolveLocaleFromSignals,
} from "./lib/locale-routing";
import { runtimeRewriteDestination } from "./config/runtime-urls";
import { isOfficialMarketingHost } from "./lib/public-host";

// Clerk public routes — no authentication required
const clerkPublicRoutes = createRouteMatcher([
  "/",
  "/login(.*)",
  "/signup(.*)",
  "/sign-in(.*)",
  "/sign-up(.*)",
  "/sso-callback(.*)",
  "/oauth/google(.*)",
  "/auth/callback",
  "/api/webhooks(.*)",
  "/api/config",
  "/api/health",
  "/pricing",
  "/docs(.*)",
  "/legal(.*)",
  "/changelog",
]);

// Old workspace-scoped route segments that existed before the URL refactor
// (pre-#1131). Any URL with these as the FIRST segment is a legacy URL that
// needs to be rewritten to /{slug}/{route}/... so old bookmarks, deep links,
// and post-revert-and-reapply users don't hit 404.
const LEGACY_ROUTE_SEGMENTS = new Set([
  "issues",
  "projects",
  "agents",
  "teams",
  "inbox",
  "my-issues",
  "automations",
  "runtimes",
  "skills",
  "settings",
  "usage",
]);

function resolveLocale(req: NextRequest): string {
  return resolveLocaleFromSignals({
    cookieLocale: req.cookies.get(LOCALE_COOKIE)?.value,
    acceptLanguage: req.headers.get("accept-language"),
  });
}

// Forward the resolved locale to RSC layouts via the `x-patchbay-locale`
// request header. layout.tsx reads it through `await headers()`. The
// `request: { headers }` form is what makes the header land on the upstream
// request — without it the value would only sit on the response.
function nextWithLocale(req: NextRequest): NextResponse {
  const headers = new Headers(req.headers);
  headers.set(PATCHBAY_LOCALE_HEADER, resolveLocale(req));
  return NextResponse.next({ request: { headers } });
}

function runtimeRewrite(req: NextRequest): NextResponse | null {
  const { pathname } = req.nextUrl;

  const runtimeDestination = runtimeRewriteDestination(pathname, process.env);
  if (runtimeDestination) {
    const url = new URL(runtimeDestination);
    url.search = req.nextUrl.search;
    return NextResponse.rewrite(url);
  }

  return null;
}

// Next.js 16 renamed `middleware` → `proxy`. API surface (NextRequest /
// NextResponse / cookies / matcher) is identical; the only behavioral
// change is the runtime — proxy is forced to nodejs and cannot opt into
// edge.
const clerkProxy = clerkMiddleware(async (auth, req) => {
  const { pathname } = req.nextUrl;

  if (!clerkPublicRoutes(req)) {
    const { userId } = await auth();
    if (!userId) {
      const loginUrl = req.nextUrl.clone();
      loginUrl.pathname = "/login";
      loginUrl.search = "";
      loginUrl.searchParams.set("redirect_url", `${pathname}${req.nextUrl.search}`);
      return NextResponse.redirect(loginUrl);
    }
  }

  const hasSession =
    req.cookies.has("patchbay_logged_in") ||
    req.cookies.has("cordy_logged_in"); // legacy-brand-compat
  const lastSlug = req.cookies.get("last_workspace_slug")?.value;

  // --- Legacy URL redirect: /issues/... → /{slug}/issues/... ---
  // Old bookmarks and clients that hit us before the slug migration would
  // otherwise 404 since the route moved under [workspaceSlug].
  const firstSegment = pathname.split("/")[1] ?? "";
  if (LEGACY_ROUTE_SEGMENTS.has(firstSegment)) {
    const url = req.nextUrl.clone();

    if (!hasSession) {
      url.pathname = "/login";
      return NextResponse.redirect(url);
    }

    if (lastSlug) {
      // Preserve deep-link path + query: /issues/abc → /{lastSlug}/issues/abc
      url.pathname = `/${lastSlug}${pathname}`;
      return NextResponse.redirect(url);
    }

    // Logged-in but no cookie yet (never opened a workspace, or the cookie was
    // cleared). Root is the wrong destination: the root-path rule below leaves
    // `/` on the public site for the official marketing hosts even with a
    // session, so bouncing there dead-ends on the landing page instead of
    // reaching the app. /login already resolves an authenticated visitor
    // against their workspace list — including pending invitations and the
    // no-workspace-yet case — and replaces to the right destination. Deep-link
    // path and query are dropped rather than passed as `next`: they are legacy
    // segments themselves, so feeding one back would land here again.
    url.pathname = "/login";
    url.search = "";
    return NextResponse.redirect(url);
  }

  // --- Root path: redirect logged-in users to their last workspace ---
  // The official cloud host also serves the public marketing site. Visiting
  // https://patchbay.ai/ must remain a public-site navigation even when a local
  // desktop/runtime session has fresh auth cookies; explicit app routes such
  // as /acme/issues and legacy /issues still route to the workspace app.
  if (
    pathname === "/" &&
    hasSession &&
    lastSlug &&
    !isOfficialMarketingHost(req.nextUrl.hostname)
  ) {
    const url = req.nextUrl.clone();
    url.pathname = `/${lastSlug}/issues`;
    return NextResponse.redirect(url);
  }

  // --- Default: forward locale header to RSC, no redirect/rewrite ---
  // Covers logged-out root path, /login, /:slug/*, and everything else.
  return nextWithLocale(req);
});

export function proxy(
  req: NextRequest,
  event?: NextFetchEvent,
): ReturnType<typeof clerkProxy> {
  const rewrite = runtimeRewrite(req);
  if (rewrite) return rewrite;

  return clerkProxy(
    req,
    event ?? ({ waitUntil: () => undefined } as unknown as NextFetchEvent),
  );
}

export const config = {
  // i18n header must land on every page request, so we use the standard
  // negative-lookahead pattern from Next's i18n guide, plus explicit runtime
  // proxy routes whose upstream origins are resolved from process.env at
  // request time instead of being baked into next.config.js at build time.
  matcher: [
    "/api/:path*",
    "/auth/:path*",
    "/uploads/:path*",
    "/docs/:path*",
    "/ws",
    "/((?!api|_next/static|_next/image|favicon.ico|.*\\.).*)",
  ],
};
