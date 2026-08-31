export function AuthShell({ children }: { children: React.ReactNode }) {
  return (
    <main className="auth-page">
      <section className="auth-card" aria-labelledby="auth-brand">
        <div id="auth-brand" className="auth-brand">
          Patchbay
        </div>
        {children}
      </section>
    </main>
  );
}
