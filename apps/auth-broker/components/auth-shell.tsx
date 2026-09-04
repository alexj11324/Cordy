/** White Clerk surface on the left, Patchbay identity on the black right. */
export function AuthShell({ children }: { children: React.ReactNode }) {
  return (
    <main
      data-testid="accounts-auth-shell"
      className="accounts-auth-shell"
    >
      <section
        data-testid="accounts-auth-form-panel"
        className="accounts-auth-form-panel"
      >
        {children}
      </section>
      <aside
        data-testid="accounts-auth-brand-panel"
        className="accounts-auth-brand-panel"
        aria-hidden="true"
      >
        <div className="accounts-brand-lockup">
          <img
            data-testid="patchbay-mark"
            src="/icons/icon.svg"
            alt=""
            width="112"
            height="112"
          />
          <span>Patchbay</span>
        </div>
      </aside>
    </main>
  );
}
