import { createRef, type ComponentProps, type MutableRefObject } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { I18nProvider } from "@patchbay/core/i18n/react";
import { configureShortcutPlatform, useShortcutStore } from "@patchbay/core/shortcuts";
import type { ContentEditorRef } from "../../editor/content-editor";
import enCommon from "../../locales/en/common.json";
import enEditor from "../../locales/en/editor.json";
import { LexicalComposerEditor } from "./lexical-composer-editor";

const editorApi = vi.hoisted(() => {
  let markdown = "";
  const instance = {
    blur: vi.fn(),
    cleanDocument: vi.fn(() => {
      markdown = "";
    }),
    dispatchCommand: vi.fn(),
    focus: vi.fn(),
    getDocument: vi.fn(() => markdown),
    getLexicalEditor: vi.fn(() => ({})),
    setDocument: vi.fn((_type: string, content: string) => {
      markdown = content;
    }),
  };
  return {
    instance,
    inited: false,
    reset() {
      markdown = "";
      this.inited = false;
      instance.blur.mockClear();
      instance.cleanDocument.mockClear();
      instance.dispatchCommand.mockClear();
      instance.focus.mockClear();
      instance.getDocument.mockClear();
      instance.getLexicalEditor.mockClear();
      instance.setDocument.mockClear();
      instance.getDocument.mockImplementation(() => markdown);
      instance.setDocument.mockImplementation((_type: string, content: string) => {
        markdown = content;
      });
    },
    setMarkdown(value: string) {
      markdown = value;
    },
  };
});

const editorProps = vi.hoisted(() => ({
  last: null as null | Record<string, unknown>,
}));

const searchIssuesMock = vi.hoisted(() => vi.fn());

vi.mock("@patchbay/core/platform", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@patchbay/core/platform")>();
  return { ...actual, getCurrentWsId: () => "ws-1" };
});

vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: { getState: () => ({ user: { id: "u1" } }) },
}));

vi.mock("@patchbay/core/api", () => ({
  api: {
    searchIssues: (...args: unknown[]) => searchIssuesMock(...args),
    searchProjects: vi.fn().mockResolvedValue({ projects: [], total: 0 }),
  },
}));

vi.mock("@lobehub/editor", () => ({
  INSERT_MENTION_COMMAND: { type: "INSERT_MENTION_COMMAND" },
  ReactImagePlugin: () => null,
  ReactMentionPlugin: () => null,
}));

vi.mock("@lobehub/editor/react", () => ({
  useEditor: () => editorApi.instance,
  Editor: (props: Record<string, unknown>) => {
    editorProps.last = props;
    if (!editorApi.inited) {
      editorApi.inited = true;
      (props.onInit as ((editor: unknown) => void) | undefined)?.(editorApi.instance);
    }
    return <div data-testid="lobe-editor" />;
  },
}));

const TEST_RESOURCES = { en: { common: enCommon, editor: enEditor } };

function renderEditor(
  props: Partial<ComponentProps<typeof LexicalComposerEditor>> = {},
  ref?: MutableRefObject<ContentEditorRef | null>,
) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <I18nProvider locale="en" resources={TEST_RESOURCES}>
        <LexicalComposerEditor ref={ref} placeholder="Message" {...props} />
      </I18nProvider>
    </QueryClientProvider>,
  );
}

describe("LexicalComposerEditor", () => {
  beforeEach(() => {
    editorApi.reset();
    editorProps.last = null;
    searchIssuesMock.mockReset();
    searchIssuesMock.mockResolvedValue({ issues: [], total: 0 });
    useShortcutStore.getState().resetAll();
    configureShortcutPlatform("windows");
  });

  afterEach(() => {
    useShortcutStore.getState().resetAll();
    configureShortcutPlatform(null);
  });

  it("mounts LobeHub's chat-variant Editor with mention and slash options", () => {
    renderEditor();
    expect(screen.getByTestId("lexical-composer-editor")).toHaveAttribute(
      "data-editor-engine",
      "lexical",
    );
    expect(editorProps.last?.variant).toBe("chat");
    expect(editorProps.last?.mentionOption).toBeTruthy();
    expect(editorProps.last?.slashOption).toBeTruthy();
  });

  it("sends on Mod+Enter and ignores Shift+Enter", () => {
    const onSubmit = vi.fn();
    renderEditor({ onSubmit });
    const onPressEnter = editorProps.last?.onPressEnter as (args: {
      event: KeyboardEvent;
    }) => boolean | void;

    expect(
      onPressEnter({
        event: new KeyboardEvent("keydown", { key: "Enter", shiftKey: true }),
      }),
    ).not.toBe(true);
    expect(onSubmit).not.toHaveBeenCalled();

    expect(
      onPressEnter({
        event: new KeyboardEvent("keydown", { key: "Enter" }),
      }),
    ).not.toBe(true);
    expect(onSubmit).not.toHaveBeenCalled();

    expect(
      onPressEnter({
        event: new KeyboardEvent("keydown", { key: "Enter", ctrlKey: true }),
      }),
    ).toBe(true);
    expect(onSubmit).toHaveBeenCalledTimes(1);
  });

  it("exposes the ContentEditorRef contract used by ChatInput uploads", () => {
    const ref = createRef<ContentEditorRef>();
    renderEditor({}, ref as MutableRefObject<ContentEditorRef | null>);
    expect(ref.current?.insertUploadPlaceholder({ uploadId: "u1", filename: "a.png" })).toBe(
      true,
    );
    expect(ref.current?.hasActiveUploads()).toBe(true);
    editorApi.setMarkdown("hello");
    expect(ref.current?.getMarkdown()).toBe("hello");
    act(() => {
      ref.current?.clearContent();
    });
    expect(editorApi.instance.cleanDocument).toHaveBeenCalled();
  });

  it("converts over-threshold pastes into a file upload", () => {
    const onUploadFile = vi.fn().mockResolvedValue(null);
    renderEditor({ onUploadFile, pasteAsFileThreshold: 8 });
    const text = "0123456789";
    fireEvent.paste(screen.getByTestId("lexical-composer-editor"), {
      clipboardData: {
        files: [],
        getData: (type: string) => (type === "text/plain" ? text : ""),
      },
    });
    expect(onUploadFile).toHaveBeenCalledTimes(1);
    expect(onUploadFile.mock.calls[0]?.[0]).toBeInstanceOf(File);
    expect((onUploadFile.mock.calls[0]?.[0] as File).name).toBe("pasted-text.txt");
  });

  it("does not replace the Lexical document when value echoes the live markdown", () => {
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    function Harness({ value }: { value: string }) {
      return (
        <QueryClientProvider client={queryClient}>
          <I18nProvider locale="en" resources={TEST_RESOURCES}>
            <LexicalComposerEditor placeholder="Message" value={value} />
          </I18nProvider>
        </QueryClientProvider>
      );
    }

    const { rerender } = render(<Harness value="" />);
    editorApi.setMarkdown("hello from caret");
    editorApi.instance.setDocument.mockClear();
    rerender(<Harness value="hello from caret" />);
    expect(editorApi.instance.setDocument).not.toHaveBeenCalled();
  });

  it("does not emit onUpdate synchronously from insertMarkdownAtEnd", () => {
    const onUpdate = vi.fn();
    const ref = createRef<ContentEditorRef>();
    renderEditor({ onUpdate, debounceMs: 10_000 }, ref as MutableRefObject<ContentEditorRef | null>);
    editorApi.setMarkdown("body");
    act(() => {
      expect(ref.current?.insertMarkdownAtEnd("![shot.png](/api/attachments/a/download)")).toBe(
        true,
      );
    });
    expect(onUpdate).not.toHaveBeenCalled();
  });

  it("focuses on init when a non-zero focusRequest is already pending", () => {
    renderEditor({ focusRequest: 1 });
    expect(editorApi.instance.focus).toHaveBeenCalled();
  });

  it("does not steal focus on init when focusRequest is inert", () => {
    renderEditor({ focusRequest: 0 });
    expect(editorApi.instance.focus).not.toHaveBeenCalled();
  });

  it("asks the server for mention matches that are not in the query cache", async () => {
    searchIssuesMock.mockResolvedValue({
      issues: [
        {
          id: "i-1007",
          identifier: "PB-1007",
          title: "Closed issue",
          status: "done",
        },
      ],
      total: 1,
    });
    renderEditor();
    const mentionOption = editorProps.last?.mentionOption as {
      items: (search: { matchingString: string } | null) => Promise<Array<{ key: string }>>;
    };

    const items = await mentionOption.items({ matchingString: "协作" });
    expect(searchIssuesMock).toHaveBeenCalledWith(
      expect.objectContaining({ q: "协作", include_closed: true }),
    );
    expect(items.some((item) => item.key === "issue:i-1007")).toBe(true);
  });
});
