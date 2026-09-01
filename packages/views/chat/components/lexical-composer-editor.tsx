"use client";

import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
  type ClipboardEvent as ReactClipboardEvent,
} from "react";
import {
  INSERT_MENTION_COMMAND,
  ReactImagePlugin,
  ReactMentionPlugin,
  type IEditor,
  type ISlashMenuOption,
  type ISlashOption,
} from "@lobehub/editor";
import { Editor, useEditor } from "@lobehub/editor/react";
import { useQueryClient } from "@tanstack/react-query";
import { configStore } from "@patchbay/core/config";
import { getShortcut } from "@patchbay/core/shortcuts";
import { createSafeId, isImeComposing } from "@patchbay/core/utils";
import { getCurrentWsId } from "@patchbay/core/platform";
import { cn } from "@patchbay/ui/lib/utils";
import type { ContentEditorRef } from "../../editor/content-editor";
import type { MentionItem } from "../../editor/extensions/mention-suggestion";
import {
  listMentionSuggestionItems,
} from "../../editor/extensions/mention-suggestion";
import { recordMentionUsage } from "../../editor/extensions/mention-recency";
import {
  buildChatSkillItems,
} from "../../editor/extensions/slash-command-suggestion";
import {
  markPastedTextFile,
  PASTED_TEXT_FILENAME,
} from "../../editor/extensions/file-upload";
import { preprocessMarkdown } from "../../editor/utils/preprocess";
import { shouldHandleSubmitShortcut } from "../../editor/extensions/submit-shortcut";
import { useT } from "../../i18n";
import {
  mentionChipLabel,
  serializeComposerMention,
} from "./lexical-composer-markdown";
import type { UploadResult } from "@patchbay/core/hooks/use-file-upload";

type PendingUpload = {
  uploadId: string;
  filename: string;
  size?: number;
};

type LexicalComposerEditorProps = {
  value?: string;
  onUpdate?: (markdown: string, baseMarkdown: string) => void;
  placeholder?: string;
  className?: string;
  debounceMs?: number;
  onSubmit?: () => void;
  onUploadFile?: (file: File, uploadId: string) => Promise<UploadResult | null>;
  pasteAsFileThreshold?: number;
  onUploadingChange?: (uploading: boolean) => void;
  mentionMode?: "default" | "context";
  mentionContextItems?: MentionItem[];
  enableSlashCommands?: boolean;
};

function normalizeMarkdown(md: string): string {
  return md.trim();
}

function readEditorMarkdown(editor: IEditor | null | undefined): string {
  if (!editor?.getLexicalEditor()) return "";
  for (const type of ["markdown", "text"] as const) {
    try {
      const doc = editor.getDocument(type) as unknown;
      if (typeof doc === "string") return doc;
    } catch {
      // Data source is registered only after plugins mount.
    }
  }
  return "";
}

function writeEditorMarkdown(
  editor: IEditor,
  markdown: string,
  keepHistory: boolean,
): boolean {
  if (!editor.getLexicalEditor()) return false;
  const prepared = preprocessMarkdown(markdown, {
    cdnDomain: configStore.getState().cdnDomain,
  });
  try {
    editor.setDocument("markdown", prepared, { keepHistory });
    return true;
  } catch {
    try {
      editor.setDocument("text", prepared, { keepHistory });
      return true;
    } catch {
      return false;
    }
  }
}

function attachmentMarkdownFromResult(result: UploadResult): string {
  const link = result.markdownLink || result.link;
  return (result.content_type ?? "").startsWith("image/")
    ? `![${result.filename}](${link})`
    : `[${result.filename}](${link})`;
}

function appendEditorMarkdown(editor: IEditor, fragment: string): boolean {
  const current = readEditorMarkdown(editor).replace(/(\n\s*)+$/, "");
  const next = current ? `${current}\n\n${fragment}` : fragment;
  return writeEditorMarkdown(editor, next, true);
}

const LexicalComposerEditor = forwardRef<
  ContentEditorRef,
  LexicalComposerEditorProps
>(function LexicalComposerEditor(
  {
    value = "",
    onUpdate,
    placeholder = "",
    className,
    debounceMs = 100,
    onSubmit,
    onUploadFile,
    pasteAsFileThreshold,
    onUploadingChange,
    mentionMode = "default",
    mentionContextItems,
    enableSlashCommands = true,
  },
  ref,
) {
  const { t } = useT("editor");
  const queryClient = useQueryClient();
  const editor = useEditor();
  const [pendingUploads, setPendingUploads] = useState<PendingUpload[]>([]);
  const pendingRef = useRef<PendingUpload[]>([]);
  const editorReadyRef = useRef(false);
  const focusOnReadyRef = useRef(false);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined,
  );
  const lastEmittedRef = useRef<string | null>(null);
  const lastSyncedValueRef = useRef(value);
  const documentBaseRef = useRef(normalizeMarkdown(value));
  const onUpdateRef = useRef(onUpdate);
  const onSubmitRef = useRef(onSubmit);
  const onUploadFileRef = useRef(onUploadFile);
  const onUploadingChangeRef = useRef(onUploadingChange);
  const pasteAsFileThresholdRef = useRef(pasteAsFileThreshold);
  const mentionContextItemsRef = useRef(mentionContextItems ?? []);
  const valueRef = useRef(value);

  onUpdateRef.current = onUpdate;
  onSubmitRef.current = onSubmit;
  onUploadFileRef.current = onUploadFile;
  onUploadingChangeRef.current = onUploadingChange;
  pasteAsFileThresholdRef.current = pasteAsFileThreshold;
  mentionContextItemsRef.current = mentionContextItems ?? [];
  valueRef.current = value;

  const syncUploading = useCallback((next: PendingUpload[]) => {
    pendingRef.current = next;
    onUploadingChangeRef.current?.(next.length > 0);
  }, []);

  const setPending = useCallback(
    (updater: (prev: PendingUpload[]) => PendingUpload[]) => {
      setPendingUploads((prev) => {
        const next = updater(prev);
        syncUploading(next);
        return next;
      });
    },
    [syncUploading],
  );

  const emitMarkdown = useCallback(
    (markdown: string) => {
      const normalized = normalizeMarkdown(markdown);
      if (normalized === lastEmittedRef.current) return;
      const base = documentBaseRef.current;
      lastEmittedRef.current = normalized;
      documentBaseRef.current = normalized;
      onUpdateRef.current?.(markdown, base);
    },
    [],
  );

  const scheduleUpdate = useCallback(() => {
    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => {
      debounceRef.current = undefined;
      emitMarkdown(readEditorMarkdown(editor));
    }, debounceMs);
  }, [debounceMs, editor, emitMarkdown]);

  const mentionMarkdownWriter = useCallback(
    (mention: { label: string; metadata?: Record<string, unknown> }) =>
      serializeComposerMention(mention),
    [],
  );

  const mentionItems = useCallback(
    async (
      search: {
        leadOffset: number;
        matchingString: string;
        replaceableString: string;
      } | null,
    ): Promise<ISlashOption[]> => {
      const query = search?.matchingString ?? "";
      const items = listMentionSuggestionItems(queryClient, query, {
        mode: mentionMode,
        getContextItems: () => mentionContextItemsRef.current,
      });
      return items.map((item): ISlashMenuOption => ({
        key: `${item.type}:${item.id}`,
        label: mentionChipLabel(item.type, item.label),
        extra: item.description,
        disabled: Boolean(item.disabledReason),
        metadata: {
          id: item.id,
          type: item.type,
          label: item.label,
          disabledReason: item.disabledReason,
        },
      }));
    },
    [mentionMode, queryClient],
  );

  const mentionOnSelect = useCallback((ed: IEditor, option: ISlashMenuOption) => {
    const metadata = option.metadata ?? {};
    if (metadata.disabledReason) return;
    const type = typeof metadata.type === "string" ? metadata.type : "member";
    const id = typeof metadata.id === "string" ? metadata.id : String(option.key);
    const label =
      typeof metadata.label === "string" ? metadata.label : String(option.label);
    const wsId = getCurrentWsId();
    if (wsId) {
      recordMentionUsage(wsId, { id, label, type } as MentionItem);
    }
    ed.dispatchCommand(INSERT_MENTION_COMMAND, {
      label: mentionChipLabel(type, label),
      metadata: { id, type, label },
    });
  }, []);

  const mentionOption = useMemo(
    () => ({
      items: mentionItems,
      markdownWriter: mentionMarkdownWriter,
      maxLength: 50,
      onSelect: mentionOnSelect,
    }),
    [mentionItems, mentionMarkdownWriter, mentionOnSelect],
  );

  const slashItems = useCallback(
    async (
      search: {
        leadOffset: number;
        matchingString: string;
        replaceableString: string;
      } | null,
    ): Promise<ISlashOption[]> => {
      const query = search?.matchingString ?? "";
      const skills = buildChatSkillItems(queryClient, query);
      if (skills.length === 0) {
        return [
          {
            key: "__empty",
            label: query.trim()
              ? t(($) => $.slash_command.no_results)
              : t(($) => $.slash_command.no_skills_configured),
            disabled: true,
          },
        ];
      }
      return skills.map((skill): ISlashMenuOption => ({
        key: skill.id,
        label: `/${skill.label}`,
        extra: skill.description,
        metadata: { id: skill.id, type: "skill", label: skill.label },
        onSelect: (ed) => {
          ed.dispatchCommand(INSERT_MENTION_COMMAND, {
            label: mentionChipLabel("skill", skill.label),
            metadata: { id: skill.id, type: "skill", label: skill.label },
          });
        },
      }));
    },
    [queryClient, t],
  );

  const slashOption = useMemo(
    () => (enableSlashCommands ? { items: slashItems } : undefined),
    [enableSlashCommands, slashItems],
  );

  const insertPending = useCallback(
    (upload: PendingUpload): boolean => {
      if (pendingRef.current.some((item) => item.uploadId === upload.uploadId)) {
        return true;
      }
      setPending((prev) =>
        prev.some((item) => item.uploadId === upload.uploadId)
          ? prev
          : [...prev, upload],
      );
      return true;
    },
    [setPending],
  );

  const settlePending = useCallback(
    (uploadId: string, result: UploadResult): boolean => {
      if (!pendingRef.current.some((item) => item.uploadId === uploadId)) {
        return false;
      }
      setPending((prev) => prev.filter((item) => item.uploadId !== uploadId));
      if (!editorReadyRef.current) return false;
      const md = attachmentMarkdownFromResult(result);
      const landed = appendEditorMarkdown(editor, md);
      if (landed) emitMarkdown(readEditorMarkdown(editor));
      return landed;
    },
    [editor, emitMarkdown, setPending],
  );

  const runUpload = useCallback(
    async (file: File) => {
      const handler = onUploadFileRef.current;
      if (!handler) return;
      const uploadId = createSafeId();
      insertPending({
        uploadId,
        filename: file.name,
        size: file.size,
      });
      try {
        const result = await handler(file, uploadId);
        if (!editorReadyRef.current) return;
        if (result) settlePending(uploadId, result);
        else setPending((prev) => prev.filter((item) => item.uploadId !== uploadId));
      } catch {
        setPending((prev) => prev.filter((item) => item.uploadId !== uploadId));
      }
    },
    [insertPending, settlePending, setPending],
  );

  const handlePasteCapture = useCallback(
    (event: ReactClipboardEvent<HTMLDivElement>) => {
      const native = event.nativeEvent;
      const files = native.clipboardData?.files;
      if (files && files.length > 0) {
        if (!onUploadFileRef.current) return;
        event.preventDefault();
        event.stopPropagation();
        Array.from(files).forEach((file) => {
          void runUpload(file);
        });
        return;
      }
      const threshold = pasteAsFileThresholdRef.current;
      if (!threshold || threshold <= 0 || !onUploadFileRef.current) return;
      const text = native.clipboardData?.getData("text/plain") ?? "";
      if (text.length <= threshold) return;
      event.preventDefault();
      event.stopPropagation();
      const file = markPastedTextFile(
        new File([text], PASTED_TEXT_FILENAME, { type: "text/plain" }),
        text,
      );
      void runUpload(file);
    },
    [runUpload],
  );

  const handleSubmitKey = useCallback(
    (event: KeyboardEvent): boolean => {
      if (
        !shouldHandleSubmitShortcut(event, {
          configuredShortcut: getShortcut("send"),
          composing: isImeComposing(event),
        })
      ) {
        return false;
      }
      onSubmitRef.current?.();
      return true;
    },
    [],
  );

  const handleInit = useCallback(
    (instance: IEditor) => {
      editorReadyRef.current = true;
      const incoming = valueRef.current;
      if (incoming) {
        writeEditorMarkdown(instance, incoming, false);
      }
      lastEmittedRef.current = normalizeMarkdown(readEditorMarkdown(instance));
      lastSyncedValueRef.current = incoming;
      if (focusOnReadyRef.current) {
        focusOnReadyRef.current = false;
        instance.focus();
      }
    },
    [],
  );

  useEffect(() => {
    if (!editorReadyRef.current) return;
    if (value === lastSyncedValueRef.current) return;
    lastSyncedValueRef.current = value;
    if (pendingRef.current.length > 0) return;
    const current = readEditorMarkdown(editor);
    const isDirty =
      lastEmittedRef.current !== null &&
      normalizeMarkdown(current) !== lastEmittedRef.current;
    if (isDirty) return;
    writeEditorMarkdown(editor, value, false);
    lastEmittedRef.current = normalizeMarkdown(readEditorMarkdown(editor));
    documentBaseRef.current = normalizeMarkdown(value);
  }, [editor, value]);

  useEffect(() => {
    return () => {
      editorReadyRef.current = false;
      if (debounceRef.current) clearTimeout(debounceRef.current);
    };
  }, []);

  useImperativeHandle(ref, () => ({
    getMarkdown: () => readEditorMarkdown(editor),
    clearContent: () => {
      editor.cleanDocument();
      lastEmittedRef.current = "";
      documentBaseRef.current = "";
    },
    focus: () => {
      if (editorReadyRef.current) editor.focus();
      else focusOnReadyRef.current = true;
    },
    focusAtCoords: () => {
      if (editorReadyRef.current) editor.focus();
      else focusOnReadyRef.current = true;
    },
    focusAtAnchor: () => {
      if (editorReadyRef.current) editor.focus();
      else focusOnReadyRef.current = true;
    },
    blur: () => {
      editor.blur();
    },
    uploadFile: (file: File) => {
      void runUpload(file);
    },
    hasActiveUploads: () => pendingRef.current.length > 0,
    insertUploadPlaceholder: (upload) => insertPending(upload),
    settleUploadPlaceholder: (uploadId, result) => settlePending(uploadId, result),
    insertMarkdownAtEnd: (markdown: string) => {
      if (!editorReadyRef.current) return false;
      const landed = appendEditorMarkdown(editor, markdown);
      if (landed) emitMarkdown(readEditorMarkdown(editor));
      return landed;
    },
    flushPendingUpdate: () => {
      if (!debounceRef.current) return null;
      clearTimeout(debounceRef.current);
      debounceRef.current = undefined;
      const md = normalizeMarkdown(readEditorMarkdown(editor));
      if (md === lastEmittedRef.current) return null;
      lastEmittedRef.current = md;
      documentBaseRef.current = md;
      return md;
    },
    adoptContent: (markdown: string) => {
      lastSyncedValueRef.current = markdown;
      writeEditorMarkdown(editor, markdown, false);
      lastEmittedRef.current = normalizeMarkdown(readEditorMarkdown(editor));
      documentBaseRef.current = normalizeMarkdown(markdown);
    },
  }));

  return (
    <div
      data-testid="lexical-composer-editor"
      data-editor-engine="lexical"
      className={cn("relative min-h-9 w-full text-body [&_p]:mb-0", className)}
      onPasteCapture={handlePasteCapture}
    >
      <Editor
        className="min-h-9"
        content=""
        debounceWait={0}
        editor={editor}
        enablePasteMarkdown
        mentionOption={mentionOption}
        onInit={handleInit}
        onKeyDown={({ event }) => {
          if (event.key === "Enter") return undefined;
          if (handleSubmitKey(event)) return true;
          return undefined;
        }}
        onPressEnter={({ event }) => {
          if (handleSubmitKey(event)) return true;
          return undefined;
        }}
        onTextChange={scheduleUpdate}
        placeholder={placeholder}
        plugins={[ReactMentionPlugin, ReactImagePlugin]}
        slashOption={slashOption}
        type="text"
        variant="chat"
      />
      {pendingUploads.length > 0 ? (
        <ul className="mt-1 flex flex-col gap-1 text-caption text-muted-foreground">
          {pendingUploads.map((upload) => (
            <li key={upload.uploadId}>
              {t(($) => $.file_card.uploading, { filename: upload.filename })}
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
});

export { LexicalComposerEditor };
