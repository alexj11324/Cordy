import type { ReactNode } from "react";
import { PatchbayIcon } from "@patchbay/ui/components/common/patchbay-icon";

/**
 * The approved shadcn authentication composition: a dark form panel on the
 * left and a black brand panel on the right. Small screens keep the form
 * usable by collapsing the decorative panel.
 */
export function AuthShell({ children }: { children: ReactNode }) {
  return (
    <main
      data-testid="auth-shell"
      className="grid min-h-dvh w-full bg-zinc-950 md:grid-cols-2"
    >
      <section className="grid min-h-dvh overflow-y-auto bg-zinc-950 p-6 text-white [place-items:safe_center] md:p-10">
        {children}
      </section>
      <aside
        data-testid="auth-brand-panel"
        className="relative hidden min-h-dvh overflow-hidden bg-zinc-950 text-white md:flex md:items-center md:justify-center"
        aria-hidden="true"
      >
        <div className="absolute inset-0 bg-[radial-gradient(circle_at_50%_35%,rgba(255,255,255,0.12),transparent_42%)]" />
        <PatchbayIcon className="relative size-24 text-white" noSpin />
      </aside>
    </main>
  );
}
