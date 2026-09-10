"use client";

import { useMemo, useState } from "react";
import type { IssueStatus, UpdateIssueRequest } from "@orvilo/core/types";
import { STATUS_CONFIG } from "@orvilo/core/issues/config";
import { useIssueStatuses } from "@orvilo/core/issue-statuses/hooks";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { StatusIcon } from "../status-icon";
import { PropertyPicker, PickerItem } from "./property-picker";
import { useT } from "../../../i18n";
import { useStatusLabel } from "../../utils/status-label";
import { useStatusOptions } from "../../utils/status-options";
import { ReviewSubmissionDialog } from "../review-submission-dialog";

/** Above this many options the flat list stops being scannable. */
const SEARCH_THRESHOLD = 9;

export function StatusPicker({
  status,
  onUpdate,
  onReviewSubmit,
  trigger: customTrigger,
  triggerRender,
  open: controlledOpen,
  onOpenChange: controlledOnOpenChange,
  align,
}: {
  /**
   * The currently-selected status, used to check the matching row. `null`
   * means "no single current value" (e.g. a batch selection spanning several
   * statuses) — no row is checked. Single-issue callers always pass a concrete
   * status.
   */
  status: IssueStatus | null;
  onUpdate: (updates: Partial<UpdateIssueRequest>) => void;
  onReviewSubmit: (
    updates: Partial<UpdateIssueRequest>,
  ) => Promise<boolean>;
  trigger?: React.ReactNode;
  triggerRender?: React.ReactElement;
  open?: boolean;
  onOpenChange?: (v: boolean) => void;
  align?: "start" | "center" | "end";
}) {
  const [internalOpen, setInternalOpen] = useState(false);
  const open = controlledOpen ?? internalOpen;
  const setOpen = controlledOnOpenChange ?? setInternalOpen;
  const [query, setQuery] = useState("");
  const [reviewStatus, setReviewStatus] = useState<IssueStatus | null>(null);
  const { t } = useT("issues");
  // Every StatusPicker call site lives inside the workspace shell (issue
  // detail, table, board batch toolbar, create-issue modal), so the provider
  // is guaranteed here.
  const wsId = useWorkspaceId();
  const { categoryOf, colorOf } = useIssueStatuses(wsId);
  const labelOf = useStatusLabel(wsId);


  const allOptions = useStatusOptions(wsId);

  const options = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return allOptions;
    return allOptions.filter((o) => o.label.toLowerCase().includes(q));
  }, [allOptions, query]);

  const searchable = allOptions.length > SEARCH_THRESHOLD;

  return (
    <><PropertyPicker
      open={open}
      onOpenChange={(v) => {
        if (!v) setQuery("");
        setOpen(v);
      }}
      width="w-52"
      align={align}
      triggerRender={triggerRender}
      searchable={searchable}
      searchPlaceholder={t(($) => $.filters.search_status)}
      onSearchChange={setQuery}
      trigger={
        customTrigger ??
        (status != null ? (
          <>
            <StatusIcon
              status={status}
              category={categoryOf(status)}
              color={colorOf(status)}
              className="h-3.5 w-3.5 shrink-0"
            />
            <span className="truncate">{labelOf(status)}</span>
          </>
        ) : null)
      }
    >
      {options.map((option) => (
        <PickerItem
          key={option.key}
          selected={option.key === status}
          hoverClassName={STATUS_CONFIG[option.category].hoverBg}
          onClick={() => {
            if (option.category === "in_review" && (status == null || categoryOf(status) !== "in_review")) {
              setReviewStatus(option.key);
            } else {
              onUpdate({ status: option.key });
            }
            setOpen(false);
            setQuery("");
          }}
        >
          <StatusIcon
            status={option.key}
            category={option.category}
            color={option.color}
            className="h-3.5 w-3.5"
          />
          <span className="truncate">{option.label}</span>
        </PickerItem>
      ))}
    </PropertyPicker>
    {reviewStatus && (
      <ReviewSubmissionDialog
        status={reviewStatus}
        onClose={() => setReviewStatus(null)}
        onSubmit={onReviewSubmit}
      />
    )}
    </>
  );
}
