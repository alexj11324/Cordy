"use client";

import { useAuthMessages } from "@/lib/auth-messages";

/** The Accounts surface mirrors the shadcn authentication example. */
export function AuthShell({ children }: { children: React.ReactNode }) {
  const messages = useAuthMessages();

  return (
    <main data-testid="accounts-auth-shell" className="accounts-auth-shell">
      <aside
        data-testid="accounts-auth-brand-panel"
        data-panel-tone="charcoal"
        className="accounts-auth-brand-panel accounts-auth-brand-panel--left"
      >
        <div className="accounts-brand-lockup">
          <img
            data-testid="patchbay-mark"
            src="/icons/icon.svg"
            alt=""
            width="28"
            height="28"
          />
          <span>{messages.brand}</span>
        </div>
        <blockquote className="accounts-brand-quote">{messages.quote}</blockquote>
      </aside>
      <section
        data-testid="accounts-auth-form-panel"
        data-panel-tone="black"
        className="accounts-auth-form-panel accounts-auth-form-panel--right"
      >
        <span className="accounts-auth-login-label">{messages.login}</span>
        <div className="accounts-auth-form-content">{children}</div>
      </section>
    </main>
  );
}
