"use client";

import { useState } from "react";
import type { IssueStatus, UpdateIssueRequest } from "@orvilo/core/types";
import { Button } from "@orvilo/ui/components/ui/button";
import { Input } from "@orvilo/ui/components/ui/input";
import { Textarea } from "@orvilo/ui/components/ui/textarea";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@orvilo/ui/components/ui/dialog";
import { ReviewerPicker } from "./pickers/executor-picker";
import { useT } from "../../i18n";

export function ReviewSubmissionDialog({
  status,
  onClose,
  onSubmit,
}: {
  status: IssueStatus;
  onClose: () => void;
  onSubmit: (patch: Partial<UpdateIssueRequest>) => Promise<boolean>;
}) {
  const { t } = useT("issues");
  const [worktree, setWorktree] = useState("");
  const [branch, setBranch] = useState("");
  const [commit, setCommit] = useState("");
  const [prs, setPRs] = useState("");
  const [reviewer, setReviewer] = useState<Partial<UpdateIssueRequest>>({});
  const [submitting, setSubmitting] = useState(false);
  const urls = prs
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  const valid = Boolean(
    worktree.trim() &&
      branch.trim() &&
      /^[0-9a-f]{40}([0-9a-f]{24})?$/i.test(commit.trim()) &&
      urls.length > 0 &&
      reviewer.reviewer_id,
  );

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !submitting) onClose();
      }}
    >
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{t(($) => $.review_submission.title)}</DialogTitle>
          <DialogDescription>
            {t(($) => $.review_submission.description)}
          </DialogDescription>
        </DialogHeader>
        <form
          className="space-y-4"
          onSubmit={async (event) => {
            event.preventDefault();
            if (!valid || submitting) return;
            setSubmitting(true);
            let accepted = false;
            try {
              accepted = await onSubmit({
                status,
                ...reviewer,
                review_submission: {
                  worktree: worktree.trim(),
                  branch: branch.trim(),
                  commit: commit.trim(),
                  pull_requests: urls,
                },
              });
            } catch {
              // The mutation owner reports the concrete error. Keep this
              // dialog mounted with its local draft so the user can retry.
              accepted = false;
            } finally {
              setSubmitting(false);
            }
            if (accepted) onClose();
          }}
        >
          <fieldset className="contents" disabled={submitting}>
            <label className="block space-y-1.5 text-body">
              <span>{t(($) => $.review_submission.worktree)}</span>
              <Input
                value={worktree}
                onChange={(event) => setWorktree(event.target.value)}
                required
              />
            </label>
            <label className="block space-y-1.5 text-body">
              <span>{t(($) => $.review_submission.branch)}</span>
              <Input
                value={branch}
                onChange={(event) => setBranch(event.target.value)}
                required
              />
            </label>
            <label className="block space-y-1.5 text-body">
              <span>{t(($) => $.review_submission.commit)}</span>
              <Input
                value={commit}
                onChange={(event) => setCommit(event.target.value)}
                required
              />
            </label>
            <label className="block space-y-1.5 text-body">
              <span>{t(($) => $.review_submission.pull_requests)}</span>
              <Textarea
                value={prs}
                onChange={(event) => setPRs(event.target.value)}
                required
              />
              <span className="text-caption text-muted-foreground">
                {t(($) => $.review_submission.pr_hint)}
              </span>
            </label>
            <div className="space-y-1.5">
              <p className="text-body">{t(($) => $.detail.prop_reviewer)}</p>
              <ReviewerPicker
                reviewerType={reviewer.reviewer_type ?? null}
                reviewerId={reviewer.reviewer_id ?? null}
                onUpdate={setReviewer}
              />
            </div>
            <DialogFooter>
              <Button type="button" variant="ghost" onClick={onClose}>
                {t(($) => $.review_submission.cancel)}
              </Button>
              <Button type="submit" disabled={!valid || submitting}>
                {t(($) => $.review_submission.submit)}
              </Button>
            </DialogFooter>
          </fieldset>
        </form>
      </DialogContent>
    </Dialog>
  );
}
