# Orvilo rollout

The approved monochrome symbol and Orvilo display names are the product
identity in app chrome, authentication, app icons, documentation, and
localized UI. Workspace packages are `@orvilo/*`; the Go module is
`github.com/orvilo-ai/orvilo/server`.

Live machine identity that existing installs and production still depend on
stays in place: the GitHub repository `alexj11324/Cordy`, the `orvilo` CLI
binary and release artifact names, `orvilo://` callback schemes, existing
desktop data directories, `X-Orvilo-*` API headers, cookies, Compose project
names, Helm chart name, container image names, and service URLs such as
`orvilo.aspectlylabs.com`.

Web and Accounts now use @orvilo/auth-ui, extracted from the existing custom
Accounts form. Prebuilt Clerk SignIn/SignUp imports, their alias pages, and the
Clerk theme dependency were removed. Web performs same-origin Google SSO and
waits for the existing Clerk-to-Go exchange before its post-login redirect.

## Registration gate

Live production Clerk configuration observed during this task uses email_code
but auth_password.required=true. The custom email-only form cannot supply a
required password. Proposed narrow configuration change: auth_password.required
became false with user approval on 2026-09-05, preserving password support and
every other security setting. Read-back confirmed required=false, email_code,
and verify_at_sign_up=true. The verification_code email template is delivered
by Clerk. Actual inbox delivery and live new-account acceptance are pending;
do not treat UI rendering as proof that signup completes.

## Deployment

The existing Aspectlylabs production workflow builds backend, web, docs and
broker together after CI succeeds on main. It deploys an immutable manifest
through the server deployment handler and verifies service build identity and
browser authentication. Target retains accounts.aspectlylabs.com,
orvilo.aspectlylabs.com and api.aspectlylabs.com, and the existing Clerk
instance and database. No new domain or independent account store is created.

Production application deployment and the database were inspected read-only.
Clerk registration configuration was updated as recorded above; no new
application release or database change was made by this work. Merge/deployment and a real registration plus
login-to-Go-API round trip remain required before claiming seamless rollout.
