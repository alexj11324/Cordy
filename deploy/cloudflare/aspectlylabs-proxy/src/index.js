import enAuth from "../../../../packages/views/locales/en/auth.json";
import jaAuth from "../../../../packages/views/locales/ja/auth.json";
import koAuth from "../../../../packages/views/locales/ko/auth.json";
import zhAuth from "../../../../packages/views/locales/zh-Hans/auth.json";

const CALLBACK_MESSAGES = JSON.stringify({
  en: enAuth.callback,
  ja: jaAuth.callback,
  ko: koAuth.callback,
  "zh-Hans": zhAuth.callback,
}).replace(/</g, "\\u003c");

const HTML = `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>Sign in · Patchbay</title>
  <style>
    html, body {
      margin: 0;
      height: 100%;
      min-height: 100dvh;
    }
    body {
      display: grid;
      place-items: safe center;
      min-height: 100dvh;
      width: 100%;
      background:
        radial-gradient(1200px 600px at 18% 8%, rgba(108, 71, 255, 0.16), transparent 58%),
        radial-gradient(900px 480px at 92% 92%, rgba(59, 130, 246, 0.12), transparent 52%),
        #eef0f6;
      font-family: "Source Sans Pro", ui-sans-serif, system-ui, sans-serif;
    }
    #app {
      width: fit-content;
      max-width: calc(100% - 1.5rem);
    }
  </style>
</head>
<body>
  <div id="app"></div>
  <script
    defer
    crossorigin="anonymous"
    src="https://clerk.aspectlylabs.com/npm/@clerk/ui@1/dist/ui.browser.js"
    type="text/javascript"></script>
  <script
    defer
    crossorigin="anonymous"
    data-clerk-publishable-key="pk_live_Y2xlcmsuYXNwZWN0bHlsYWJzLmNvbSQ"
    src="https://clerk.aspectlylabs.com/npm/@clerk/clerk-js@6/dist/clerk.browser.js"
    type="text/javascript"></script>
  <script>
    window.addEventListener("load", async function () {
      await Clerk.load({
        ui: { ClerkUI: window.__internal_ClerkUICtor },
      });

      const app = document.getElementById("app");
      const path = location.pathname.replace(/\\/$/, "") || "/";
      const APP_ORIGIN = "https://www.aspectlylabs.com";
      const APP_BASE_PATH = "/patchbay";
      const APP_HOME = APP_ORIGIN + APP_BASE_PATH + "/";
      const ALLOWED_APP_ORIGINS = new Set([APP_ORIGIN]);
      const isSsoCallback =
        path === "/sso-callback" ||
        path === "/login/sso-callback" ||
        path === "/signup/sso-callback" ||
        path === "/sign-in/sso-callback" ||
        path === "/sign-up/sso-callback";
      const isLegacyCallback = path === "/auth/callback";
      const isDesktopHandoff =
        new URL(location.href).searchParams.get("platform") === "desktop";
      const callbackMessages = ${CALLBACK_MESSAGES};
      const appearance = {
        elements: {
          rootBox: { width: "fit-content", height: "auto", margin: "0 auto" },
        },
      };

      function resolveRedirectTarget() {
        const requested = new URL(location.href).searchParams.get("redirect_url");
        if (!requested) return APP_HOME;

        try {
          const target = new URL(requested, APP_HOME);
          if (
            target.username ||
            target.password ||
            !ALLOWED_APP_ORIGINS.has(target.origin) ||
            (target.pathname !== APP_BASE_PATH &&
              !target.pathname.startsWith(APP_BASE_PATH + "/"))
          ) {
            return APP_HOME;
          }
          return target.href;
        } catch {
          return APP_HOME;
        }
      }

      function redirectToApp(target) {
        window.location.replace(target);
      }

      function redirectToSignIn(target) {
        const signInUrl = new URL("/sign-in", location.origin);
        if (isDesktopHandoff) {
          signInUrl.searchParams.set("platform", "desktop");
        }
        if (target !== APP_HOME) {
          signInUrl.searchParams.set("redirect_url", target);
        }
        window.location.replace(signInUrl.href);
      }

      function hasOAuthCallbackParams() {
        const search = new URL(location.href).searchParams;
        return (
          ["code", "state", "__clerk_status", "__clerk_ticket"].some((name) =>
            search.has(name),
          ) || location.hash.length > 1
        );
      }

      function callbackLocale() {
        const languages = [
          ...(Array.isArray(navigator.languages) ? navigator.languages : []),
          navigator.language,
          "en",
        ];
        for (const language of languages) {
          const normalized = (language || "").toLowerCase();
          if (normalized.startsWith("zh")) return "zh-Hans";
          if (normalized.startsWith("ja")) return "ja";
          if (normalized.startsWith("ko")) return "ko";
          if (normalized.startsWith("en")) return "en";
        }
        return "en";
      }

      function escapeHtml(value) {
        return value.replace(
          /[&<>"']/g,
          (character) =>
            ({
              "&": "&amp;",
              "<": "&lt;",
              ">": "&gt;",
              '"': "&quot;",
              "'": "&#39;",
            })[character],
        );
      }

      function localizedCallbackMessages() {
        const locale = callbackLocale();
        document.documentElement.lang = locale;
        return callbackMessages[locale] || callbackMessages.en;
      }

      function showCallbackError(target) {
        const messages = localizedCallbackMessages();
        app.innerHTML =
          '<div role="alert" style="display:grid;gap:.75rem;max-width:22rem;padding:1.25rem;border:1px solid rgba(15,23,42,.12);border-radius:1rem;background:#fff;color:#1e293b;text-align:center;box-shadow:0 12px 36px rgba(15,23,42,.12)">' +
          "<p>" +
          escapeHtml(messages.error) +
          "</p>" +
          '<a id="retry-sign-in" href="/sign-in" style="color:#2563eb;text-decoration:underline">' +
          escapeHtml(messages.retry) +
          "</a>" +
          "</div>";
        const retryUrl = new URL("/sign-in", location.origin);
        if (isDesktopHandoff) {
          retryUrl.searchParams.set("platform", "desktop");
        }
        if (target !== APP_HOME) {
          retryUrl.searchParams.set("redirect_url", target);
        }
        document.getElementById("retry-sign-in").href = retryUrl.href;
      }

      const redirectTarget = resolveRedirectTarget();
      const desktopRedirectTarget =
        APP_ORIGIN + APP_BASE_PATH + "/login?platform=desktop";
      const postAuthTarget = isDesktopHandoff
        ? desktopRedirectTarget
        : redirectTarget;

      if (isLegacyCallback) {
        redirectToApp(postAuthTarget);
        return;
      }

      if (isSsoCallback) {
        if (Clerk.isSignedIn) {
          redirectToApp(postAuthTarget);
          return;
        }
        if (!hasOAuthCallbackParams()) {
          redirectToSignIn(redirectTarget);
          return;
        }

        const messages = localizedCallbackMessages();
        app.innerHTML =
          '<p role="status" style="padding:1rem;color:#475569">' +
          escapeHtml(messages.completing) +
          "</p>";
        try {
          await Clerk.handleRedirectCallback(
            {
              signInUrl: isDesktopHandoff
                ? "/sign-in?platform=desktop"
                : "/sign-in",
              signUpUrl: isDesktopHandoff
                ? "/sign-up?platform=desktop"
                : "/sign-up",
              signInFallbackRedirectUrl: postAuthTarget,
              signUpFallbackRedirectUrl: postAuthTarget,
            },
            async (to) => {
              window.location.assign(to);
            },
          );
        } catch (error) {
          console.error("Clerk redirect callback failed", error);
          showCallbackError(redirectTarget);
        }
        return;
      }

      if (Clerk.isSignedIn) {
        redirectToApp(postAuthTarget);
        return;
      }

      const isSignUpPath =
        path === "/sign-up" ||
        path === "/signup" ||
        path.startsWith("/sign-up/") ||
        path.startsWith("/signup/");
      if (isSignUpPath) {
        app.innerHTML = '<div id="sign-up"></div>';
        const signUpPath = path.startsWith("/signup") ? "/signup" : "/sign-up";
        Clerk.mountSignUp(document.getElementById("sign-up"), {
          routing: "path",
          path: signUpPath,
          signInUrl: isDesktopHandoff
            ? "/sign-in?platform=desktop"
            : "/sign-in",
          fallbackRedirectUrl: postAuthTarget,
          appearance: appearance,
        });
        return;
      }

      const isSignInPath =
        path === "/sign-in" || path.startsWith("/sign-in/");
      const signInPath = isSignInPath ? "/sign-in" : "/";
      app.innerHTML = '<div id="sign-in"></div>';
      Clerk.mountSignIn(document.getElementById("sign-in"), {
        routing: "path",
        path: signInPath,
        signUpUrl: isDesktopHandoff
          ? "/sign-up?platform=desktop"
          : "/sign-up",
        fallbackRedirectUrl: postAuthTarget,
        appearance: appearance,
      });
    });
  </script>
</body>
</html>
`;

export default {
  async fetch(request) {
    const url = new URL(request.url);
    if (url.pathname === "/health") {
      return new Response("ok\\n", {
        headers: { "content-type": "text/plain; charset=utf-8" },
      });
    }
    return new Response(HTML, {
      headers: {
        "content-type": "text/html; charset=utf-8",
        "cache-control": "no-store",
      },
    });
  },
};
