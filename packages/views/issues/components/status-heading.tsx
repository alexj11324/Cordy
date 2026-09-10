import { statusCategoryOfKey } from "@orvilo/core/issues";
import type { IssueStatus } from "@orvilo/core/types";
import { StatusIcon } from "./status-icon";
import { useT } from "../../i18n";

export function StatusHeading({
  status,
  count,
}: {
  status: IssueStatus;
  count: number;
}) {
  const { t } = useT("issues");
  return (
    <div className="flex items-center gap-2">
      <span className="inline-flex items-center gap-1.5 text-caption font-medium">
        <StatusIcon status={status} className="h-4 w-4" />
        {t(($) => $.status[statusCategoryOfKey(status)])}
      </span>
      <span className="text-caption text-muted-foreground">{count}</span>
    </div>
  );
}
