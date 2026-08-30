import { FlaskConical, WifiOff } from "lucide-react";
import { useT } from "@patchbay/views/i18n";

/** Visible boundary marker for the browser-only, no-backend renderer. */
export function DesktopWebPreviewNotice() {
  const { t } = useT("common");

  return (
    <aside
      className="pointer-events-none relative z-20 mx-4 mt-2 shrink-0 self-end flex max-w-[min(34rem,calc(100%-2rem))] items-center gap-2 rounded-full border border-brand/25 bg-background/95 px-3 py-1.5 text-micro text-muted-foreground shadow-sm backdrop-blur"
      data-preview-notice="true"
      role="status"
    >
      <span className="inline-flex items-center gap-1 font-semibold text-brand">
        <FlaskConical className="size-3" aria-hidden="true" />
        {t(($) => $.preview.badge)}
      </span>
      <span className="text-border" aria-hidden="true">
        •
      </span>
      <span>{t(($) => $.preview.sample_data)}</span>
      <span className="inline-flex items-center gap-1 text-muted-foreground">
        <WifiOff className="size-3" aria-hidden="true" />
        {t(($) => $.preview.no_backend)}
      </span>
    </aside>
  );
}
