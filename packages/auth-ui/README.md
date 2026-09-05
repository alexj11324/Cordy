# Shared custom authentication form

Web and Accounts consume the same custom email-code/Google form. Clerk hooks
perform authentication; no prebuilt SignIn/SignUp card or Clerk theme is used.
Messages and form styles are owned here. Each app owns Google navigation and
post-authentication session exchange; the broker retains its desktop PKCE flow.

Broker tests exercise the shared email form. Web tests cover Google callback
URLs and prohibit prebuilt cards/theme imports. All public Web login/signup
aliases use the same LoginPage and preserve the existing redirect bindings.
