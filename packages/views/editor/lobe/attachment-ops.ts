/**
 * Imperative attachment operations on a Lexical editor.
 *
 * These are the replacements for the TipTap-side helpers in
 * `packages/views/editor/extensions/file-upload.ts`, and they preserve that
 * module's externally-visible contract — including the parts that look odd
 * until you know what depends on them. Each oddity is called out at its site.
 *
 * Every function that mutates takes an `editor` and runs its own update, so
 * callers never nest updates. The `$`-prefixed helpers must run inside one.
 */

import { INSERT_MARKDOWN_COMMAND } from "@lobehub/editor";
import {
  $createParagraphNode,
  $createRangeSelection,
  $getRoot,
  $isElementNode,
  $isRootOrShadowRoot,
  $setSelection,
  type LexicalEditor,
  type LexicalNode,
} from "lexical";
import { $wrapNodeInElement } from "@lexical/utils";

import {
  $createAttachmentNode,
  $isAttachmentNode,
  type AttachmentNode,
  type AttachmentStatus,
} from "./attachment-node";
import type { AttachmentSettleResult } from "./attachment-plugin";

/** Depth-first walk, because attachments sit inside a paragraph (see insert). */
function $collectAttachments(): AttachmentNode[] {
  const found: AttachmentNode[] = [];
  const visit = (node: LexicalNode): void => {
    if ($isAttachmentNode(node)) {
      found.push(node);
      return;
    }
    if ($isElementNode(node)) node.getChildren().forEach(visit);
  };
  $getRoot().getChildren().forEach(visit);
  return found;
}

/** Runs `fn` in an update and returns what it captured. */
function capture<T>(editor: LexicalEditor, fn: () => T): T {
  let captured!: T;
  editor.update(
    () => {
      captured = fn();
    },
    { discrete: true },
  );
  return captured;
}

/** The node holding this upload, or null. Safe to call outside an update. */
export function findAttachment(
  editor: LexicalEditor,
  clientUploadId: string,
): AttachmentNode | null {
  return capture(
    editor,
    () =>
      $collectAttachments().find(
        (node) => node.clientUploadId === clientUploadId,
      ) ?? null,
  );
}

/**
 * Append an attachment node at the end of the document.
 *
 * Wrapped in a paragraph when it lands directly on the root, matching
 * LobeHub's `FilePlugin`. Without the wrap, a block-level decorator becomes a
 * bare root child and the surrounding paragraph structure the markdown writer
 * expects no longer holds.
 */
export function appendAttachment(
  editor: LexicalEditor,
  payload: {
    clientUploadId: string;
    filename: string;
    fileSize?: number;
    kind?: "file" | "image";
    previewSrc?: string;
  },
): void {
  capture(editor, () => {
    const node = $createAttachmentNode({ ...payload, status: "pending" });
    $getRoot().append(node);
    if ($isRootOrShadowRoot(node.getParentOrThrow())) {
      $wrapNodeInElement(node, $createParagraphNode);
    }
  });
}

/**
 * Draw a placeholder for an upload this document is not showing yet.
 *
 * **Idempotent**: an id already in the document counts as success. The caller
 * retries until this returns true (a reopened composer races the editor's
 * mount), and without the early return the caller's own first successful
 * insert would be mistaken for a failure, so it would retry forever and stack
 * duplicate cards for one upload.
 *
 * Returns false only when there is genuinely nothing to do — which, unlike the
 * TipTap version, cannot happen for an un-mounted editor: Lexical either has a
 * state to mutate or the call throws.
 */
export function insertAttachmentPlaceholder(
  editor: LexicalEditor,
  upload: { filename: string; size?: number; uploadId: string },
): boolean {
  return capture(editor, () => {
    const existing = $collectAttachments().some(
      (node) => node.clientUploadId === upload.uploadId,
    );
    if (existing) return true;

    // No preview: the bytes went with the mount that started the upload, so a
    // card is the only thing that can be drawn honestly.
    const node = $createAttachmentNode({
      clientUploadId: upload.uploadId,
      filename: upload.filename,
      fileSize: upload.size ?? 0,
      kind: "file",
      status: "pending",
    });
    $getRoot().append(node);
    if ($isRootOrShadowRoot(node.getParentOrThrow())) {
      $wrapNodeInElement(node, $createParagraphNode);
    }
    return true;
  });
}

/**
 * Turn a placeholder into the finished attachment, in place.
 *
 * False when this document holds no node for the id — the caller then falls
 * back to appending the link, which is how an upload that settled while a
 * *different* mount was showing the draft still reaches the visible document.
 */
export function settleAttachment(
  editor: LexicalEditor,
  clientUploadId: string,
  result: AttachmentSettleResult,
): boolean {
  return capture(editor, () => {
    const node = $collectAttachments().find(
      (candidate) => candidate.clientUploadId === clientUploadId,
    );
    if (!node) return false;
    node.setUploaded(result);
    return true;
  });
}

/**
 * Mark an upload as failed, in place.
 *
 * Exists so callers never reach for the node to call `setError` themselves:
 * Lexical only permits node mutation inside an update, and a caller holding a
 * node reference from `findAttachment` would be mutating it from the outside,
 * which throws at runtime rather than at compile time. Symmetric with
 * {@link settleAttachment} for that reason.
 */
export function failAttachment(
  editor: LexicalEditor,
  clientUploadId: string,
  message: string,
): boolean {
  return capture(editor, () => {
    const node = $collectAttachments().find(
      (candidate) => candidate.clientUploadId === clientUploadId,
    );
    if (!node) return false;
    node.setError(message);
    return true;
  });
}

/** Drop the node for an upload. Returns whether one was there to drop. */
export function removeAttachment(
  editor: LexicalEditor,
  clientUploadId: string,
): boolean {
  return capture(editor, () => {
    const node = $collectAttachments().find(
      (candidate) => candidate.clientUploadId === clientUploadId,
    );
    if (!node) return false;
    const parent = node.getParent();
    node.remove();
    // The wrap paragraph would otherwise survive as an empty block, leaving a
    // blank line behind where a failed upload used to be.
    if (parent && $isElementNode(parent) && parent.getChildrenSize() === 0) {
      parent.remove();
    }
    return true;
  });
}

/**
 * Whether any upload is still in flight.
 *
 * This is the gate the send affordance reads. It must count only `pending`:
 * an errored node is finished business (the coordinator removes it), and
 * counting it would block sending forever.
 */
export function hasActiveUploads(editor: LexicalEditor): boolean {
  return capture(editor, () =>
    $collectAttachments().some((node) => node.status === "pending"),
  );
}

/**
 * Append a markdown fragment to the end of the document, parsed.
 *
 * Routed through the kernel's own `INSERT_MARKDOWN_COMMAND` rather than
 * concatenating text and re-setting the whole document: the command runs the
 * real markdown parser, so headings, lists and links arrive as nodes. The
 * read-concatenate-write alternative would also destroy the user's selection
 * and undo history on every upload that settles.
 *
 * The caret is parked at the end first — the command inserts at the selection,
 * and the write-back path fires from a promise where the caret can be anywhere
 * (or nowhere).
 *
 * Returns whether the kernel handled it, which is the caller's signal to fall
 * back to a different delivery path.
 */
export function insertMarkdownAtEnd(
  editor: LexicalEditor,
  markdown: string,
): boolean {
  capture(editor, () => {
    const last = $getRoot().getLastChild();
    if (!$isElementNode(last)) return;
    const selection = $createRangeSelection();
    // `element` offsets address children, so "size" is the position after the
    // last child — the end of the block.
    selection.anchor.set(last.getKey(), last.getChildrenSize(), "element");
    selection.focus.set(last.getKey(), last.getChildrenSize(), "element");
    $setSelection(selection);
  });

  return editor.dispatchCommand(INSERT_MARKDOWN_COMMAND, {
    historyState: null,
    markdown,
  });
}

/** Every attachment currently in the document, with its status. */
export function listAttachments(
  editor: LexicalEditor,
): Array<{ clientUploadId: string; status: AttachmentStatus }> {
  return capture(editor, () =>
    $collectAttachments().map((node) => ({
      clientUploadId: node.clientUploadId,
      status: node.status,
    })),
  );
}
