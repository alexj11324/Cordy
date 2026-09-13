"use client";

import { useEffect, useState } from "react";
import { Button, Input, Modal } from "@lobehub/ui/base-ui";
import { isImeComposing } from "@orvilo/core/utils";
import { useT } from "../../i18n";

/**
 * Typed-confirmation dialog for workspace deletion — GitHub's repo-delete
 * pattern. The destructive button stays disabled until the user types
 * the workspace name exactly (case-sensitive, no trimming). The friction
 * is deliberate: deleting a workspace cascades into every issue, agent,
 * skill, and run under it, and the backend has no soft-delete.
 *
 * Case-sensitive match matches GitHub's pattern and catches the "I
 * remember the gist of the name but not the casing" misfire. No trim —
 * leading/trailing whitespace indicates a typo, and silently accepting
 * it would weaken the whole point of the gate.
 *
 * Input value resets whenever the dialog closes so reopening doesn't
 * leak the previous attempt (which might have been for a different
 * workspace after a swap).
 *
 * **The shadcn `Dialog` became Lobe's base-ui `Modal`, and three of the
 * conversion's details are load-bearing:**
 *
 * 1. **The footer is supplied, not defaulted.** `Modal`'s footer is
 *    `cancelBtnNode + okBtnNode` unless the caller passes one
 *    (`es/base-ui/Modal/Modal.mjs`), and its OK button is wired to `onOk` —
 *    which this dialog does not use, because the commit point is a typed
 *    confirmation rather than a generic confirm. Leaving `footer` undefined
 *    would render a Cancel and a dead OK that dismisses nothing. Both buttons
 *    are passed explicitly, so the destructive tone and the `Deleting…` label
 *    swap stay this file's.
 * 2. **No `destroyOnHidden`.** base-ui's `Modal` destructures a fixed prop list
 *    with no rest spread and drops it silently, even though
 *    `ModalComponentProps` declares it. It would also be the wrong tool: the
 *    draft lives in this component, not in the Modal's children.
 * 3. **The label stays a plain `<label htmlFor>`.** The mapping deletes the
 *    shadcn `Label` atom, but the association is what names the input, so it
 *    moves onto a real `<label>` element — which is also what the migrated
 *    dialogs in this directory do (`labels-tab`, `account-tab`). The control is
 *    a Lobe `Input`; its `id` lands on the inner `<input>`, through the
 *    wrapper's `...rest`.
 *
 * `width={512}` is the old `DialogContent`'s `sm:max-w-lg`; there was no
 * `DialogFooter` and no separator band, so the footer keeps Lobe's own padding
 * and background.
 */
export function DeleteWorkspaceDialog({
  workspaceName,
  loading = false,
  open,
  onOpenChange,
  onConfirm,
}: {
  workspaceName: string;
  loading?: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}) {
  const { t } = useT("settings");
  const [typed, setTyped] = useState("");
  const matched = typed === workspaceName;

  // Reset on close (so reopening for a different workspace doesn't leak
  // the prior attempt) AND on workspaceName change (if another owner
  // renames the workspace while the dialog is open, the already-typed
  // string stops matching and there'd be no feedback explaining why).
  useEffect(() => {
    setTyped("");
  }, [open, workspaceName]);

  const submit = () => {
    if (!matched || loading) return;
    onConfirm();
  };

  return (
    <Modal
      footer={
        <>
          {/* The baseline was `variant="outline"` — Lobe's bordered default, so
              nothing is passed for it. `htmlType` defaults to `"button"`, so
              the old `type="button"` has no equivalent to carry either. */}
          <Button
            disabled={loading}
            onClick={() => onOpenChange(false)}
          >
            {t(($) => $.delete_workspace_dialog.cancel)}
          </Button>
          {/* `variant="destructive"` is `type="primary"` + `danger`, the solid
              red `dangerSolid` treatment. The label swap is kept rather than
              Lobe's `loading` slot, because the swap is what a screen reader
              reads back as the state. */}
          <Button
            danger
            disabled={!matched || loading}
            type="primary"
            onClick={submit}
          >
            {loading ? t(($) => $.delete_workspace_dialog.deleting) : t(($) => $.delete_workspace_dialog.confirm)}
          </Button>
        </>
      }
      open={open}
      title={t(($) => $.delete_workspace_dialog.title)}
      width={512}
      onCancel={() => onOpenChange(false)}
    >
      <div className="space-y-4">
        {/* Lobe's `Modal` has no `description` prop, so the old
            `DialogDescription` is the body's first line. */}
        <p className="text-body text-muted-foreground">
          {t(($) => $.delete_workspace_dialog.description)}
        </p>

        <div className="space-y-2">
          <label className="text-caption" htmlFor="delete-workspace-confirm">
            {t(($) => $.delete_workspace_dialog.type_to_confirm_prefix)}{" "}
            <code className="rounded bg-muted px-1 py-0.5 font-mono text-caption">
              {workspaceName}
            </code>{" "}
            {t(($) => $.delete_workspace_dialog.type_to_confirm_suffix)}
          </label>
          <Input
            autoCapitalize="off"
            autoComplete="off"
            autoCorrect="off"
            autoFocus
            disabled={loading}
            id="delete-workspace-confirm"
            placeholder={workspaceName}
            spellCheck={false}
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
            onKeyDown={(e) => {
              if (isImeComposing(e)) return;
              if (e.key === "Enter") {
                e.preventDefault();
                submit();
              }
            }}
          />
        </div>
      </div>
    </Modal>
  );
}
