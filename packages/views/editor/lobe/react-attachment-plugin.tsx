"use client";

/**
 * React binding for {@link AttachmentPlugin}, plus the card the node renders.
 *
 * Split from the plugin itself on purpose: the plugin is kernel-side and
 * testable headlessly, while everything here needs a React tree. LobeHub draws
 * the same line (`FilePlugin` vs `ReactFilePlugin`).
 *
 * Mount this INSIDE the editor's React context — it reaches the kernel through
 * `useLexicalComposerContext`, which only resolves under `EditorProvider`.
 */

import {
  useLexicalComposerContext,
  type IEditor,
} from "@lobehub/editor";
import {
  $createNodeSelection,
  $setSelection,
  CLICK_COMMAND,
  COMMAND_PRIORITY_LOW,
} from "lexical";
import type { LexicalEditor } from "lexical";
import { useCallback, useEffect, useLayoutEffect, useRef } from "react";

import { AttachmentNode } from "./attachment-node";
import { attachmentPlugin } from "./attachment-plugin";

/** Shared chrome so the three states read as one component. */
const CARD_BASE =
  "inline-flex max-w-full items-center gap-2 rounded-md border px-2 py-1 text-caption";

/**
 * One attachment in the document.
 *
 * Plain markup on the product's own tokens rather than LobeHub's card
 * components: this is an Orvilo surface and should read like the rest of the
 * app, not like a transplanted chat client.
 */
function AttachmentDecorator({
  editor,
  node,
}: {
  editor: LexicalEditor;
  node: AttachmentNode;
}) {
  const ref = useRef<HTMLElement>(null);

  // Decorator nodes are skipped by caret navigation, so without an explicit
  // node selection the card can be seen but never selected — and therefore
  // never deleted from the keyboard.
  //
  // Done with Lexical's public selection API rather than LobeHub's
  // `useLexicalNodeSelection`, which the package does not export (its
  // `exports` map has no wildcard, so the deep path is closed too).
  const onClick = useCallback(
    (payload: MouseEvent) => {
      if (payload.target !== ref.current) return false;
      editor.update(() => {
        const selection = $createNodeSelection();
        selection.add(node.getKey());
        $setSelection(selection);
      });
      return true;
    },
    [editor, node],
  );

  useEffect(
    () => editor.registerCommand(CLICK_COMMAND, onClick, COMMAND_PRIORITY_LOW),
    [editor, onClick],
  );

  if (node.status === "pending") {
    return (
      <span
        className={`${CARD_BASE} border-surface-border bg-surface-raised text-muted-foreground`}
        data-attachment-status="pending"
        ref={ref as React.Ref<HTMLSpanElement>}
      >
        <span
          aria-hidden
          className="size-3 shrink-0 animate-spin rounded-full border border-current border-t-transparent"
        />
        <span className="truncate">{node.filename}</span>
      </span>
    );
  }

  if (node.status === "error") {
    return (
      <span
        className={`${CARD_BASE} border-warning/40 bg-surface-raised text-warning`}
        data-attachment-status="error"
        ref={ref as React.Ref<HTMLSpanElement>}
        title={node.message}
      >
        <span className="truncate">{node.filename}</span>
        {node.message ? <span className="truncate opacity-80">— {node.message}</span> : null}
      </span>
    );
  }

  if (node.kind === "image") {
    return (
      // eslint-disable-next-line @next/next/no-img-element -- editor content, not a page asset
      <img
        alt={node.filename}
        className="max-w-full rounded-md"
        data-attachment-status="uploaded"
        ref={ref as React.Ref<HTMLImageElement>}
        src={node.href}
      />
    );
  }

  return (
    <a
      className={`${CARD_BASE} border-surface-border bg-surface-raised text-foreground underline-offset-2 hover:underline`}
      data-attachment-status="uploaded"
      href={node.href}
      ref={ref as React.Ref<HTMLAnchorElement>}
      rel="noreferrer"
      target="_blank"
    >
      <span className="truncate">{node.filename}</span>
    </a>
  );
}

/**
 * Registers {@link AttachmentPlugin} on the enclosing editor.
 *
 * Renders nothing itself — the visual arrives through the decorator it
 * registers. Keyed on `editor` alone: re-registering on every render would
 * stack duplicate node registrations.
 */
export function ReactAttachmentPlugin({ editor: kernelEditor }: { editor?: IEditor } = {}) {
  const [editor] = useLexicalComposerContext();
  // Prefer the kernel handle from context; the optional prop is only an escape
  // hatch for a caller that already holds one.
  const target = editor ?? kernelEditor;

  useLayoutEffect(() => {
    if (!target) return;
    target.registerPlugin(attachmentPlugin, {
      decorator: (node, lexicalEditor) => (
        <AttachmentDecorator editor={lexicalEditor} node={node} />
      ),
    });
  }, [target]);

  return null;
}
