"use client";

/**
 * The Lexical-backed editor, mid-migration from the TipTap `ContentEditor`.
 *
 * It deliberately does NOT declare `ContentEditorRef`. That interface is the
 * target, but some of its members depend on machinery that is not ported yet —
 * `focusAtCoords` / `focusAtAnchor`, the mention and slash pipelines, and
 * `pasteAsFileThreshold`. Declaring the full interface and filling the gap
 * with no-ops would typecheck everywhere and fail silently at runtime, which
 * is the one outcome worth avoiding. So this declares exactly what it
 * implements, and grows as ports land.
 *
 * Must be rendered inside `LobeThemeBridge`.
 */

import type { IEditor } from "@lobehub/editor";
import { Editor, EditorProvider, useEditor } from "@lobehub/editor/react";
import { getShortcut } from "@orvilo/core/shortcuts";
import { createSafeId } from "@orvilo/core/utils";
import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useRef,
} from "react";

import {
  appendAttachment,
  hasActiveUploads,
  insertAttachmentPlaceholder,
  insertMarkdownAtEnd,
  removeAttachment,
  settleAttachment,
} from "./attachment-ops";
import { shouldHandleSubmitShortcut } from "../extensions/submit-shortcut";
import { ReactAttachmentPlugin } from "./react-attachment-plugin";
import { toSettleResult, type UploadResultLike } from "./upload-result";

/**
 * The imperative surface this editor offers today.
 *
 * A superset of `ComposerEditorRef` (`getMarkdown` / `focus` / `blur`), so it
 * can be handed straight to `useComposerSubmit`. The upload trio
 * (`insertUploadPlaceholder` / `settleUploadPlaceholder` /
 * `insertMarkdownAtEnd`) satisfies `CoordinatedUploadEditor` for the same
 * reason, so one handle drives both hooks. Both interfaces are declared by
 * their consumers and this side is shaped to match them. Nothing here declares
 * `implements`, so a drifting member is NOT caught in this file — it is caught
 * one call out, where the handle is handed to the hook.
 */
export interface LobeContentEditorHandle {
  /**
   * Force `markdown` into the document, bypassing the synchronised-`value`
   * guards.
   *
   * Those guards SKIP permanently rather than defer: `lastSyncedValueRef`
   * advances before they run, so a `value` they refuse is never re-applied.
   * That is right for their usual case (the host will publish another value),
   * but not for a host that must land a document exactly once — `issue-detail`
   * taking the server's version after a revision conflict, where an in-flight
   * upload would otherwise make the guard refuse the swap.
   *
   * Two differences from `setMarkdown`, both required by that caller:
   * it updates the merge base (the document really was adopted), and it does
   * not fire `onUpdate` (the text came from the server, so echoing it back
   * would look like a user edit).
   */
  adoptContent: (markdown: string) => void;
  blur: () => void;
  clearContent: () => void;
  /**
   * Cancel the pending debounced `onUpdate` and hand its markdown back instead
   * of firing it. Returns null when nothing is pending.
   *
   * For hosts that re-point ONE editor at a different destination (chat swaps
   * its draft key between sessions). A debounce armed under the old
   * destination would otherwise fire after the switch and write the old
   * document into the new one.
   */
  flushPendingUpdate: () => string | null;
  focus: () => void;
  getMarkdown: () => string;
  /** True while any attachment is still uploading. */
  hasActiveUploads: () => boolean;
  /** Append parsed markdown at the end. False when the kernel refused it. */
  insertMarkdownAtEnd: (markdown: string) => boolean;
  /**
   * Draw a placeholder for an upload this document is not showing yet.
   * Idempotent — an id already drawn counts as success, so a caller retrying
   * until it lands cannot be fooled into retrying forever.
   */
  insertUploadPlaceholder: (upload: {
    filename: string;
    size?: number;
    uploadId: string;
  }) => boolean;
  isEmpty: () => boolean;
  setMarkdown: (markdown: string) => void;
  /**
   * Turn a placeholder into the finished attachment. False when absent.
   *
   * Takes the host's upload result (`UploadResultLike` — the app's
   * `UploadResult`), NOT the node's internal `{ href }` shape, and converts
   * here with `toSettleResult`. Every other host-facing member of this editor
   * already speaks that currency (`onUploadFile` resolves one), and the
   * conversion is policy rather than a rename: `toSettleResult` picks the URL
   * that belongs in the document (`markdownLink`, over the raw storage URL). A
   * caller asked to pre-map it would have to re-decide that policy, and the
   * two decisions drift — which is how a host ends up handing this a value
   * whose `href` was never populated.
   *
   * The kind IS re-decided here, from the server's `content_type` rather than
   * from the filename the placeholder was drawn with: a reopened composer only
   * had the filename, and the server decides what an attachment actually is
   * (see `AttachmentNode.setUploaded`, which is where that rule is written
   * down). The TipTap kernel settles the same way, so neither kernel can
   * disagree with the other about what arrived.
   */
  settleUploadPlaceholder: (uploadId: string, result: UploadResultLike) => boolean;
  uploadFile: (file: File) => void;
}

export interface LobeContentEditorProps {
  className?: string;
  /**
   * Debounce for `onUpdate`, in milliseconds.
   *
   * Owned here rather than delegated to the editor's `debounceWait`, because
   * `flushPendingUpdate` has to be able to cancel the timer and read the
   * markdown back — and a timer the library holds cannot be cancelled.
   */
  debounceMs?: number;
  defaultValue?: string;
  disabled?: boolean;
  /**
   * Emit the last pending markdown when this editor unmounts, instead of
   * dropping it. For hosts whose editor dies mid-edit (a dialog closing on
   * save) and whose caller still needs the final text.
   */
  flushPendingOnUnmount?: boolean;
  /**
   * Fires when the configured send shortcut is pressed — the keyboard path to
   * submit. Its guard is shared with the chat composer; see the note on
   * `handlePressEnter` for what it protects and why it is not `onKeyDown`.
   */
  onSubmit?: () => void;
  /**
   * Fires with the document's markdown, debounced.
   *
   * The second argument is the **merge base**: the last authoritative
   * controlled value this editor actually adopted, as the raw string it was
   * given. It is not the latest prop and not our serialization — see the note
   * on `adoptedBaseRef`. Hosts that persist through a server-side merge
   * (`issue-detail`'s channel-media reconciliation) must send it; hosts that
   * only save the text can ignore it.
   */
  onUpdate?: (markdown: string, baseMarkdown: string) => void;
  /**
   * Called when the pending-upload count changes, so hosts can gate their send
   * affordance without polling.
   */
  onUploadingChange?: (uploading: boolean) => void;
  /**
   * Perform the upload. Receives the id the editor minted for this file; the
   * host MUST adopt it as the draft's `clientUploadId`, because that id is the
   * only thing that finds the node again when the upload outlives this mount.
   */
  onUploadFile?: (file: File, clientUploadId: string) => Promise<UploadResultLike | null>;
  placeholder?: string;
  /**
   * Externally synchronised markdown.
   *
   * Use only when changes from outside this editor must replace its document —
   * realtime updates, or a host that switches which document a stable editor
   * instance holds. Mutually exclusive with `defaultValue`: passing both would
   * mean two sources of truth for the initial document.
   *
   * The echo problem this solves: a controlled host writes our own `onUpdate`
   * value straight back as the next `value`. Re-applying it would be a no-op
   * at best and a cursor jump at worst, so the last value we emitted is
   * remembered and skipped.
   */
  value?: string;
}

/** Reads the document as markdown, tolerating every non-string source. */
function readMarkdown(editor: IEditor): string {
  const content = editor.getDocument("markdown");
  return typeof content === "string" ? content : "";
}

interface InnerProps extends LobeContentEditorProps {
  containerRef: React.RefObject<HTMLDivElement | null>;
  handleRef: React.Ref<LobeContentEditorHandle>;
}

function LobeContentEditorInner({
  className,
  containerRef,
  debounceMs = 300,
  defaultValue,
  disabled,
  flushPendingOnUnmount = false,
  handleRef,
  onSubmit,
  onUpdate,
  onUploadFile,
  onUploadingChange,
  placeholder,
  value,
}: InnerProps) {
  const editor = useEditor();
  // Kept in refs so a host re-rendering with new callbacks does not force the
  // editor to re-seed or re-register.
  const onUploadFileRef = useRef(onUploadFile);
  onUploadFileRef.current = onUploadFile;
  const onUploadingChangeRef = useRef(onUploadingChange);
  onUploadingChangeRef.current = onUploadingChange;
  const onSubmitRef = useRef(onSubmit);
  onSubmitRef.current = onSubmit;

  /**
   * The last markdown this editor either emitted or applied.
   *
   * Doubles as the echo filter and as the "have I already seen this" marker.
   * It is advanced BEFORE the document is written, matching the TipTap
   * editor's ordering: a `value` that gets refused must not be re-applied
   * forever, because the effect only fires when `value` *changes*, and a
   * marker left behind would make every subsequent render re-check the same
   * stale value.
   */
  const lastSyncedValueRef = useRef<string | null>(value ?? defaultValue ?? null);

  /**
   * The merge base handed to `onUpdate`'s second argument.
   *
   * Distinct from `lastSyncedValueRef` above, and the distinction is load
   * bearing — they look alike and are updated at different times on purpose:
   *
   *   - `lastSyncedValueRef` tracks the last value we *saw*, so the sync effect
   *     can recognise our own edit echoed back as the next `value`.
   *   - this tracks the last value we *adopted*, so the host can tell a server
   *     which markers the editor had when it produced the current text.
   *
   * It is only written when an external value is actually applied. It is NOT
   * written when the guard skips one (the editor never adopted that value) and
   * NOT written when the user edits (their keystrokes are not an adoption).
   * Substituting the latest prop here is explicitly wrong: a dirty-editor
   * guard may have skipped newer server content, so the prop can name a
   * document this editor has never seen.
   *
   * It stores the raw string it was given, never our serialization. The server
   * distinguishes "the user deleted this channel media" from "a concurrent
   * write added it" by checking whether the base still carries the
   * `<!-- orvilo:channel-media:… -->` comment — and a comment is exactly what
   * serializing the visible document drops. Feeding back serialized markdown
   * as the base makes the server re-append media the user deliberately
   * removed.
   */
  const adoptedBaseRef = useRef<string>(value ?? defaultValue ?? "");

  /**
   * Set while an externally-supplied document is being written.
   *
   * Writing a value through the kernel fires the same `onChange` a keystroke
   * does, so without this an adopted document would be reported straight back
   * to the host as if the user had typed it — which for a merge-backed host
   * means a save round-trip that rewrites exactly what it just sent, and a
   * revision-conflict path that can loop.
   */
  const suppressChangeRef = useRef(false);

  /** Write an externally-supplied document without reporting it as an edit. */
  const applyExternal = useCallback(
    (markdown: string) => {
      suppressChangeRef.current = true;
      // Blank takes the clear path for the same reason `setMarkdown` does: the
      // markdown source rejects an empty root.
      if (!markdown.trim()) editor.cleanDocument();
      else editor.setDocument("markdown", markdown);
    },
    [editor],
  );
  const onUpdateWithBaseRef = useRef(onUpdate);
  onUpdateWithBaseRef.current = onUpdate;

  useEffect(() => {
    if (value === undefined) return;
    if (value === lastSyncedValueRef.current) return;
    lastSyncedValueRef.current = value;
    // Recorded BEFORE the write, and only on this path: reaching here means
    // the value was genuinely adopted.
    adoptedBaseRef.current = value;
    applyExternal(value);
  }, [applyExternal, editor, value]);
  /** Tracked in a ref: the guard runs inside a keydown, one render too early. */
  const composingRef = useRef(false);

  /**
   * Keyboard submit, through `onPressEnter` rather than `onKeyDown`.
   *
   * Two reasons, both verified against the library rather than assumed:
   *
   *  1. `isShortcutAllowedForAction` restricts `send` to exactly `Enter` and
   *     `primary+Enter`, so every legal binding has `key === "Enter"` — the
   *     Enter-only hook covers all of them.
   *  2. `onKeyDown` would silently do nothing. The library registers its
   *     handler as `if (editor && onPressEnter) return editor.registerHighCommand(...)`,
   *     with the `onKeyDown` call nested inside that same closure, so passing
   *     `onKeyDown` alone never registers the command.
   *
   * The guard covers IME composition (a Chinese/Japanese user pressing Enter
   * to commit a candidate must never submit — and `isImeComposing` adds the
   * Safari `keyCode === 229` case that a bare `isComposing` check misses),
   * Shift+Enter, and a rebindable chord. Returning true claims the key, so a
   * submit never also inserts a newline.
   */
  const handlePressEnter = useCallback(
    ({ event }: { event: KeyboardEvent }): boolean => {
      if (
        shouldHandleSubmitShortcut(event, {
          configuredShortcut: getShortcut("send"),
          composing: composingRef.current,
        })
      ) {
        onSubmitRef.current?.();
        return true;
      }
      return false;
    },
    [],
  );

  /** The debounce timer, owned here so `flushPendingUpdate` can cancel it. */
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  /**
   * The markdown captured when the timer was armed.
   *
   * The unmount flush emits THIS rather than re-reading the editor: it runs
   * during teardown, where the kernel may already be detached and reading
   * would throw or return nothing.
   */
  const pendingMarkdownRef = useRef<string | null>(null);

  const cancelPending = useCallback((): string | null => {
    if (debounceRef.current === null) return null;
    clearTimeout(debounceRef.current);
    debounceRef.current = null;
    const pending = pendingMarkdownRef.current;
    pendingMarkdownRef.current = null;
    return pending;
  }, []);

  useEffect(() => {
    // An armed timer at unmount is a hole in the draft: the edit was made but
    // never reported. Hosts that opt in get it delivered; the rest keep the
    // old behaviour of dropping it.
    return () => {
      const pending = cancelPending();
      if (flushPendingOnUnmount && pending !== null) {
        onUpdateWithBaseRef.current?.(pending, adoptedBaseRef.current);
      }
    };
  }, [cancelPending, flushPendingOnUnmount]);

  /** Last reported gate value, so we only emit on an actual change. */
  const uploadingRef = useRef(false);
  const syncUploading = useCallback(() => {
    const lexical = editor.getLexicalEditor();
    if (!lexical) return;
    const next = hasActiveUploads(lexical);
    if (next === uploadingRef.current) return;
    uploadingRef.current = next;
    onUploadingChangeRef.current?.(next);
  }, [editor]);

  const handleChange = useCallback(() => {
    syncUploading();
    // A change we caused by writing an external document is not a user edit.
    if (suppressChangeRef.current) {
      suppressChangeRef.current = false;
      return;
    }
    if (debounceRef.current !== null) clearTimeout(debounceRef.current);
    pendingMarkdownRef.current = readMarkdown(editor);
    debounceRef.current = setTimeout(() => {
      debounceRef.current = null;
      const pending = pendingMarkdownRef.current;
      pendingMarkdownRef.current = null;
      if (pending !== null) {
        // Recorded BEFORE emitting: a controlled host republishes this exact
        // string as the next `value`, and the sync effect has to recognise it
        // as our own echo rather than an external change worth applying.
        lastSyncedValueRef.current = pending;
        // The base is deliberately NOT advanced here — our own edit is not an
        // adoption of an external value. See `adoptedBaseRef`.
        onUpdateWithBaseRef.current?.(pending, adoptedBaseRef.current);
      }
    }, debounceMs);
  }, [debounceMs, editor, syncUploading]);

  /**
   * Upload one file, preserving the id handshake.
   *
   * The id is minted before the node exists and handed to the host
   * immediately — that is the whole point. An upload that settles after this
   * mount is gone is delivered by a *different* editor instance, and this id
   * is the only thing tying the two together.
   *
   * The node is inserted synchronously so the user sees the card before any
   * network work starts.
   */
  const uploadFile = useCallback(
    (file: File) => {
      const lexical = editor.getLexicalEditor();
      if (!lexical) return;

      const clientUploadId = createSafeId();
      const kind = file.type.startsWith("image/") ? "image" : "file";

      appendAttachment(lexical, {
        clientUploadId,
        filename: file.name,
        fileSize: file.size,
        kind,
      });
      syncUploading();

      const handler = onUploadFileRef.current;
      if (!handler) return;

      // Awaited rather than `.then()`-ed: the handler's return type is a
      // promise, but a host that reports its own result through the emitters
      // (and several mocks) returns nothing at all. `await undefined` resolves
      // to undefined and lands on the remove path; `undefined.then()` throws.
      void (async () => {
        try {
          const result = await handler(file, clientUploadId);
          // A null result means the host declined the upload; anything else
          // settles through `toSettleResult`, which picks the URL that belongs
          // in the document rather than re-deciding that policy here.
          if (result) {
            settleAttachment(lexical, clientUploadId, toSettleResult(result, kind));
          } else {
            removeAttachment(lexical, clientUploadId);
          }
        } catch {
          // The host owns error reporting (toast, retry). The card must not
          // linger as if the upload were still running.
          removeAttachment(lexical, clientUploadId);
        } finally {
          syncUploading();
        }
      })();
    },
    [editor, syncUploading],
  );

  useImperativeHandle(
    handleRef,
    (): LobeContentEditorHandle => ({
      adoptContent: (markdown: string) => {
        // Advance both markers: the document was adopted, and the sync effect
        // must not treat this same string as a fresh external change if the
        // host republishes it as the next `value`.
        lastSyncedValueRef.current = markdown;
        adoptedBaseRef.current = markdown;
        applyExternal(markdown);
      },
      blur: () => editor.blur(),
      clearContent: () => editor.cleanDocument(),
      flushPendingUpdate: () => cancelPending(),
      focus: () => editor.focus(),
      getMarkdown: () => readMarkdown(editor),
      hasActiveUploads: () => {
        const lexical = editor.getLexicalEditor();
        return lexical ? hasActiveUploads(lexical) : false;
      },
      insertMarkdownAtEnd: (markdown: string) => {
        const lexical = editor.getLexicalEditor();
        return lexical ? insertMarkdownAtEnd(lexical, markdown) : false;
      },
      insertUploadPlaceholder: (upload) => {
        const lexical = editor.getLexicalEditor();
        return lexical ? insertAttachmentPlaceholder(lexical, upload) : false;
      },
      isEmpty: () => editor.isEmpty,
      setMarkdown: (markdown: string) => {
        // Blank input takes the clear path: the markdown source parses via
        // `parseMarkdownToLexical("")`, which yields an empty root that Lexical
        // rejects. Same constraint that puts the seed below on the text source.
        if (!markdown.trim()) editor.cleanDocument();
        else editor.setDocument("markdown", markdown);
      },
      settleUploadPlaceholder: (uploadId, result) => {
        const lexical = editor.getLexicalEditor();
        if (!lexical) return false;
        // Re-decide the kind from the SERVER's content type. The placeholder
        // was drawn from a filename — all a reopened composer had — so letting
        // the pick-time guess stand here would contradict
        // `AttachmentNode.setUploaded` and the TipTap kernel's settle.
        const kind = (result.content_type ?? "").startsWith("image/")
          ? "image"
          : "file";
        return settleAttachment(lexical, uploadId, toSettleResult(result, kind));
      },
      uploadFile,
    }),
    [applyExternal, cancelPending, editor, uploadFile],
  );

  return (
    <div className={className} ref={containerRef}>
      <Editor
        // `content` has no default inside `Editor` — it forwards straight to
        // `setDocument(type, content)`. `type="text"` is what makes an empty
        // seed legal, because the markdown source rejects an empty root and
        // the text source always produces one paragraph. See the kernel's own
        // `cleanDocument()`, which is `setDocument("text", "")`.
        // `value` wins when the host drives the document; `defaultValue` seeds
        // an uncontrolled one. Passing neither is the ordinary empty composer.
        content={value ?? defaultValue ?? ""}
        editable={disabled !== true}
        editor={editor}
        onChange={handleChange}
        onCompositionEnd={() => {
          composingRef.current = false;
        }}
        onCompositionStart={() => {
          composingRef.current = true;
        }}
        onPressEnter={handlePressEnter}
        placeholder={placeholder}
        // Rendered INSIDE the editor's own composer context. Mounting this as
        // a sibling instead throws "cannot find a LexicalComposerContext":
        // `EditorProvider` supplies the kernel, but the Lexical composer is
        // created by `Editor` itself.
        plugins={[ReactAttachmentPlugin]}
        type="text"
      />
    </div>
  );
}

export const LobeContentEditor = forwardRef<
  LobeContentEditorHandle,
  LobeContentEditorProps
>(function LobeContentEditor(props, ref) {
  const containerRef = useRef<HTMLDivElement>(null);

  return (
    <EditorProvider>
      <LobeContentEditorInner
        {...props}
        containerRef={containerRef}
        handleRef={ref}
      />
    </EditorProvider>
  );
});
