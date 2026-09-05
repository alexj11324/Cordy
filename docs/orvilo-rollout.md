# Orvilo rollout

The approved monochrome symbol and Orvilo display names replace Patchbay in
app chrome, authentication, app icons, documentation chrome, and localized UI.
Machine identity remains stable: package names, app IDs, callback schemes,
existing desktop data directories, API headers, artifact names and service URLs.

Web and Accounts now use @patchbay/auth-ui, extracted from the existing custom
Accounts form. Prebuilt Clerk SignIn/SignUp imports, their alias pages, and the
Clerk theme dependency were removed. Web performs same-origin Google SSO and
waits for the existing Clerk-to-Go exchange before its post-login redirect.

## Registration gate

Live production Clerk configuration observed during this task uses email_code
but auth_password.required=true. The custom email-only form cannot supply a
required password. Proposed narrow configuration change: auth_password.required
becomes false, preserving password support and every other security setting.
This production configuration change has been dry-run only; user choice and
live new-account acceptance are pending. Do not treat UI rendering as proof
that signup completes.

## Deployment

The existing Aspectlylabs production workflow builds backend, web, docs and
broker together after CI succeeds on main. It deploys an immutable manifest
through the server deployment handler and verifies service build identity and
browser authentication. Target retains accounts.aspectlylabs.com,
patchbay.aspectlylabs.com and api.aspectlylabs.com, and the existing Clerk
instance and database. No new domain or independent account store is created.

Production was inspected read-only. No new production release or database
change was made by this work. Merge/deployment and a real registration plus
login-to-Go-API round trip remain required before claiming seamless rollout.
