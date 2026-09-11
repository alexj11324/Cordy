import type { ReactNode } from "react";
import { OrviloIcon } from "@orvilo/ui/components/common/orvilo-icon";

/**
 * Signed-out splash: force Pulse dark so the form stays on the inset well
 * even when the rest of the product is in light mode.
 */
export function AuthShell({ children }: { children: ReactNode }) {
  return (
    <main
      data-testid="auth-shell"
      className="dark grid min-h-dvh w-full bg-background md:grid-cols-2"
    >
      <section className="grid min-h-dvh overflow-y-auto bg-background p-6 text-foreground [place-items:safe_center] md:p-10">
        {children}
      </section>
      <aside
        data-testid="auth-brand-panel"
        className="relative hidden min-h-dvh overflow-hidden bg-card text-foreground md:flex md:items-center md:justify-center"
        aria-hidden="true"
      >
        <div className="absolute inset-0 bg-[radial-gradient(circle_at_50%_35%,rgba(255,255,255,0.12),transparent_42%)]" />
        <OrviloIcon className="relative size-24 text-foreground" noSpin />
      </aside>
    </main>
  );
}
