"use client";

/**
 * The submit gate for composers that accept attachments (MUL-4808).
 *
 * While a file is still uploading, the editor holds a `blob:` placeholder and
 * the attachment's id does not exist yet. Serializing at that moment emits
 * nothing for the placeholder (see the image / fileCard `renderMarkdown`) and
 * binds no id, so the send would succeed while the file silently disappears
 * from what the recipient gets.
 * Every action that FIXES a draft — send, create, save, and the mode switch
 * that re-serializes a draft into another form — must therefore wait.
 *
 * Two signals, because they answer different questions:
 *
 *   `uploading` — reactive, drives the rendered button (disabled / "Uploading…"
 *                 / aria-busy). Sourced from the editor document via
 *                 `onUploadingChange`, which is the actual upload queue.
 *
 *   `isBlocked()` — imperative, read INSIDE the submit handler. A rendered
 *                 disabled state is a frame behind: Cmd+Enter and Enter-on-title
 *                 bypass the button entirely, and a click can land in the same
 *                 tick an upload starts. This reads the document at invocation
 *                 time, which is the only answer that can't be stale.
 *
 * Hosts must use both. Gating the button alone leaves the shortcut paths open;
 * gating the handler alone leaves a live-looking button that silently no-ops.
 *
 * Kept free of `api` / i18n / toast imports on purpose — the failure toast
 * lives in `use-editor-upload` — so host tests can run the real gate against a
 * mocked editor without pulling the API singleton into the module graph.
 */

import { useCallback, useState, type RefObject } from "react";

/**
 * The one thing the gate needs from an editor.
 *
 * Declared here, at the consumer, rather than imported from the editor in use
 * today. This hook never touches text or attachments directly — it asks a
 * single yes/no question — so binding it to the full `ContentEditorRef` would
 * make it unusable by any editor that answers that question differently, which
 * is exactly what a kernel swap looks like. `ContentEditorRef` still satisfies
 * this structurally, so existing callers needed no change when it was narrowed.
 */
export interface UploadGateEditor {
  /** True while any attachment in this editor is mid-upload. */
  hasActiveUploads: () => boolean;
}

interface UploadGate {
  /** True while any attachment in this editor is mid-upload. Render state. */
  uploading: boolean;
  /** Pass to `<ContentEditor onUploadingChange={...} />`. */
  onUploadingChange: (uploading: boolean) => void;
  /** Submit-time truth. Call at the top of every submit/save/switch handler. */
  isBlocked: () => boolean;
}

/**
 * Reactive + submit-time upload state for one editor.
 *
 * The returned `isBlocked` reads the editor ref rather than `uploading` on
 * purpose — see the module docstring.
 */
function useUploadGate(editorRef: RefObject<UploadGateEditor | null>): UploadGate {
  const [uploading, setUploading] = useState(false);
  const isBlocked = useCallback(
    () => editorRef.current?.hasActiveUploads() === true,
    [editorRef],
  );
  return { uploading, onUploadingChange: setUploading, isBlocked };
}

export { useUploadGate, type UploadGate };
