"use client";

/**
 * Chat composer built on LobeHub's editor.
 *
 * This is stage S3 of the `ContentEditor` migration: chat first, everything
 * else after. It is deliberately narrow — it owns *editing, the send
 * affordance, and keyboard handling*, and nothing else. Attachments are NOT
 * modelled here yet; see the migration note at the bottom.
 *
 * Must be rendered inside `LobeThemeBridge`.
 */

import { getShortcut } from "@orvilo/core/shortcuts";
import {
  ChatInput,
  ChatInputActionBar,
  Editor,
  EditorProvider,
  SendButton,
  useEditor,
} from "@lobehub/editor/react";
import {
  forwardRef,
  type ReactNode,
  useCallback,
  useImperativeHandle,
  useRef,
} from "react";
import { shouldHandleSubmitShortcut } from "../../editor/extensions/submit-shortcut";

/**
 * Imperative surface for hosts that need to drive the composer from outside —
 * keyboard shortcuts, conversation starters, draft restoration.
 *
 * `blur` / `focus` / `getMarkdown` are exactly the three members of
 * `ComposerEditorRef`, so this handle can be handed to `useComposerSubmit`
 * directly.
 */
export interface LobeComposerHandle {
  /** Drop focus, so the composer stops reading as "still writing". */
  blur: () => void;
  /** Empty the document. */
  clearContent: () => void;
  focus: () => void;
  /** Current document as markdown. */
  getMarkdown: () => string;
  /** Whether the document holds no text. */
  isEmpty: () => boolean;
  /** Replace the document with the given markdown. */
  setMarkdown: (markdown: string) => void;
}

export interface LobeComposerProps {
  /** Inert: no agent selected, or the surface is otherwise unavailable. */
  disabled?: boolean;
  /** Actions rendered on the left of the action bar. */
  leftActions?: ReactNode;
  placeholder?: string;
  /** Actions rendered between the left group and the send button. */
  rightActions?: ReactNode;
  /**
   * The user asked to send — the send button was pressed, or the configured
   * send shortcut fired.
   *
   * This does NOT clear the composer. Clearing is the host's call, because the
   * repo's send contract is await-then-render: the composer keeps the text and
   * shows a locked/spinning affordance until the server accepts, so a rejected
   * send keeps the draft for retry. See `useComposerSubmit`. The host clears by
   * calling `clearContent()` on this composer's ref from its `onAccepted`.
   */
  onSubmit: () => void;
  /** Send in flight: the send button spins and stops accepting clicks. */
  submitting?: boolean;
  /**
   * A run is streaming: the send button becomes a Stop button and calls
   * `onStop`. This is LobeHub's own send/stop morph, not an extra control.
   */
  generating?: boolean;
  onStop?: () => void;
  /** Fires on every edit, debounced by the editor. */
  onUpdate?: (markdown: string) => void;
}

/** Reads the document as markdown, tolerating every non-string source. */
function readMarkdown(editor: ReturnType<typeof useEditor>): string {
  const content = editor.getDocument("markdown");
  return typeof content === "string" ? content : "";
}

export const LobeComposer = forwardRef<LobeComposerHandle, LobeComposerProps>(
  function LobeComposer(
    {
      disabled,
      generating,
      leftActions,
      onSubmit,
      onStop,
      onUpdate,
      placeholder,
      rightActions,
      submitting,
    },
    ref,
  ) {
    const editor = useEditor();
    // Composition is tracked in a ref, not state: the guard runs inside a
    // keydown handler and a state update would land a render too late to
    // matter. `isImeComposing` also inspects the event itself — this flag
    // covers the browsers where the event alone is not enough.
    const composingRef = useRef(false);

    useImperativeHandle(
      ref,
      (): LobeComposerHandle => ({
        blur: () => editor.blur(),
        clearContent: () => editor.cleanDocument(),
        focus: () => editor.focus(),
        getMarkdown: () => readMarkdown(editor),
        // The kernel tracks emptiness itself (`IEditor.isEmpty`), and its
        // notion is the right one: a document holding only an empty paragraph
        // has no text but does have content, and re-serialising to decide
        // would also make this depend on the markdown writer being lossless.
        isEmpty: () => editor.isEmpty,
        setMarkdown: (markdown: string) => {
          // Blank input takes the clear path instead of the markdown source:
          // `parseMarkdownToLexical("")` returns an empty root, which Lexical
          // rejects outright ("the editor state is empty"). This is the same
          // constraint that forces the `content=""` seed below onto the text
          // source, and a host restoring an empty draft hits it immediately.
          if (!markdown.trim()) {
            editor.cleanDocument();
            return;
          }
          editor.setDocument("markdown", markdown);
        },
      }),
      [editor],
    );

    /**
     * Keyboard submit, reusing the pure guard the TipTap extension already
     * encodes rather than re-deriving it.
     *
     * Wired through `onPressEnter`, not `onKeyDown`, for two reasons:
     *
     *  1. It is the only correct hook. `isShortcutAllowedForAction` restricts
     *     `send` to exactly `Enter` and `primary+Enter`, so every legal binding
     *     has `key === "Enter"` — the Enter-only hook covers all of them.
     *  2. `onKeyDown` would silently do nothing. The library registers its key
     *     handler as `if (editor && onPressEnter) return editor.registerHighCommand(...)`,
     *     with the `onKeyDown` call nested inside that same closure. Passing
     *     `onKeyDown` alone never registers the command, so the composer would
     *     look wired and never fire. Avoided rather than worked around.
     *
     * What the guard protects, and why each is load-bearing here:
     *
     *  - IME composition. A Chinese/Japanese user presses Enter to commit a
     *    candidate; that Enter must never send. The library filters
     *    `event.isComposing`, but `isImeComposing` additionally covers Safari,
     *    which clears that flag on the very keydown that ends composition.
     *  - Shift+Enter and a bare Enter. Both must fall through so the editor
     *    makes a newline instead of submitting.
     *  - Configurability. The chord is read from the store, never hardcoded.
     *
     * Returning `true` claims the key: the library calls `preventDefault()`,
     * so a submit never also inserts a newline.
     *
     * Note the absent `shouldReplayNativeEnter`. That guard exists in the
     * TipTap extension to compensate for Tiptap shadowing its own Shift+Enter
     * binding; Lexical inserts a soft break natively, so falling through is
     * already the correct behaviour. Porting it would be dead weight.
     */
    const handlePressEnter = useCallback(
      ({ event }: { event: KeyboardEvent }): boolean => {
        if (
          shouldHandleSubmitShortcut(event, {
            configuredShortcut: getShortcut("send"),
            composing: composingRef.current,
          })
        ) {
          onSubmit();
          return true;
        }
        return false;
      },
      [onSubmit],
    );

    return (
      <EditorProvider>
        <ChatInput
          footer={
            <ChatInputActionBar
              left={leftActions}
              right={
                <>
                  {rightActions}
                  <SendButton
                    disabled={disabled}
                    generating={generating}
                    loading={submitting}
                    onSend={onSubmit}
                    onStop={onStop}
                  />
                </>
              }
            />
          }
          resize
        >
          <Editor
            // `content` has no default inside `Editor`: it forwards straight to
            // `setDocument(type, content)`, so omitting it makes the kernel
            // dereference `undefined.root` on mount. Seeding an empty document
            // is required, not a nicety.
            //
            // `type="text"` rather than `"markdown"` is what makes that empty
            // seed legal. The markdown source parses via
            // `parseMarkdownToLexical("")`, which yields an empty root, and
            // Lexical rejects it outright. The text source always appends one
            // paragraph per line, so `""` produces a valid empty paragraph.
            //
            // Not a workaround invented here: the kernel's own
            // `cleanDocument()` is literally `setDocument("text", "")`, so the
            // library itself depends on the text source being the one that
            // tolerates empty input. Type only selects the data source, and
            // reads are independent of it, so `getDocument("markdown")` still
            // returns markdown.
            content=""
            editable={!disabled}
            editor={editor}
            onCompositionEnd={() => {
              composingRef.current = false;
            }}
            onCompositionStart={() => {
              composingRef.current = true;
            }}
            // Verbatim passthrough: `onChange` fires for selection and cursor
            // movement too, so hosts that persist drafts should debounce.
            onChange={() => onUpdate?.(readMarkdown(editor))}
            onPressEnter={handlePressEnter}
            placeholder={placeholder}
            type="text"
          />
        </ChatInput>
      </EditorProvider>
    );
  },
);
