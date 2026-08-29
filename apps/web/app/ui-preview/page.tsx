import Link from "next/link";
import { notFound } from "next/navigation";

export default function UiPreviewIndexPage() {
  if (process.env.NODE_ENV !== "development") notFound();
  return (
    <main className="mx-auto flex min-h-dvh max-w-lg flex-col justify-center gap-6 p-8">
      <h1 className="text-title-lg font-semibold">UI preview</h1>
      <p className="text-body text-muted-foreground">
        Local screens only. No login, no backend.
      </p>
      <Link className="text-body underline" href="/ui-preview/onboarding">
        Onboarding
      </Link>
      <Link className="text-body underline" href="/ui-preview/issues">
        App / Issues
      </Link>
    </main>
  );
}
