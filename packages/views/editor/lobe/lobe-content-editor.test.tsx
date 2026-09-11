//
// End-to-end mount for the Lexical-backed editor: kernel, plugin, node,
// decorator and markdown writer together in a React tree.
//
// The unit-level rules live beside their modules (`attachment-node.test.ts`,
// `attachment-ops.test.ts`, `attachment-plugin.test.ts`). What this file owns
// is that the pieces are actually wired to each other — the failure mode a
// mount alone can produce, where every module is correct and nothing runs.

import type { UploadResult } from "@orvilo/core/hooks/use-file-upload";
import { render, waitFor } from "@testing-library/react";
import { createRef } from "react";
import { describe, expect, it, vi } from "vitest";

import { LobeThemeBridge } from "../../chat/lobe/lobe-theme-bridge";
import {
  LobeContentEditor,
  type LobeContentEditorHandle,
} from "./lobe-content-editor";

function makeUploadResult(overrides: Partial<UploadResult> = {}): UploadResult {
  return {
    id: "att-1",
    url: "https://storage.example/raw.png",
    markdownLink: "https://cdn.example/durable.png",
    link: "https://storage.example/raw.png",
    markdown_url: "https://cdn.example/durable.png",
    ...overrides,
  } as UploadResult;
}

function renderEditor(
  props: Partial<Parameters<typeof LobeContentEditor>[0]> = {},
) {
  const ref = createRef<LobeContentEditorHandle>();
  const utils = render(
    <LobeThemeBridge>
      <LobeContentEditor ref={ref} {...props} />
    </LobeThemeBridge>,
  );
  return { ...utils, ref };
}

describe("LobeContentEditor", () => {
  it("mounts an editable region", () => {
    const { container } = renderEditor();

    expect(container.querySelector('[contenteditable="true"]')).toBeTruthy();
  });

  it("seeds the document from defaultValue and reads it back as markdown", async () => {
    const { ref } = renderEditor({ defaultValue: "hello **world**" });

    await waitFor(() => {
      expect(ref.current?.getMarkdown()).toContain("hello");
    });
  });

  it("reports empty for a blank document", async () => {
    const { ref } = renderEditor();

    await waitFor(() => {
      expect(ref.current?.isEmpty()).toBe(true);
    });
  });

  it("clears the document", async () => {
    const { ref } = renderEditor({ defaultValue: "some text" });
    await waitFor(() => {
      expect(ref.current?.getMarkdown()).toContain("some text");
    });

    ref.current?.clearContent();

    await waitFor(() => {
      expect(ref.current?.isEmpty()).toBe(true);
    });
  });

  it("seeds a controlled editor from value", async () => {
    const { ref } = renderEditor({ value: "from the server" });

    await waitFor(() => {
      expect(ref.current?.getMarkdown()).toContain("from the server");
    });
  });

  it("replaces the document when an external value arrives", async () => {
    // The realtime path: someone else edited the same field, or the host
    // re-pointed this instance at a different document.
    const { ref, rerender } = renderEditor({ value: "first" });
    await waitFor(() => {
      expect(ref.current?.getMarkdown()).toContain("first");
    });

    rerender(
      <LobeThemeBridge>
        <LobeContentEditor ref={ref} value="second" />
      </LobeThemeBridge>,
    );

    await waitFor(() => {
      expect(ref.current?.getMarkdown()).toContain("second");
    });
    expect(ref.current?.getMarkdown()).not.toContain("first");
  });

  it("ignores a value that repeats the last one it emitted", async () => {
    // The echo path, and the reason the sync effect keeps a marker: a
    // controlled host writes our own onUpdate straight back as the next
    // `value`. Re-applying it rewrites the document the user is still typing
    // in, which shows up as the caret jumping to the end.
    const seen: string[] = [];
    const { ref, rerender } = renderEditor({
      onUpdate: (md) => seen.push(md),
      value: "seed",
    });
    await waitFor(() => expect(ref.current).not.toBeNull());

    rerender(
      <LobeThemeBridge>
        <LobeContentEditor ref={ref} onUpdate={(md) => seen.push(md)} value="seed" />
      </LobeThemeBridge>,
    );

    // Same value, no external change — the document must be left alone.
    await waitFor(() => {
      expect(ref.current?.getMarkdown()).toContain("seed");
    });
    expect(seen).toEqual([]);
  });

  it("hands the host the id it minted for an upload", async () => {
    // THE contract MUL-5181 rests on. An upload that outlives this mount is
    // reconciled by a *different* editor instance, and the host's draft record
    // can only find the node again through the id the editor minted here. If
    // this stops arriving, every long upload silently loses its write-back.
    const seen: string[] = [];
    const onUploadFile = vi.fn(
      async (_file: File, clientUploadId: string): Promise<UploadResult | null> => {
        seen.push(clientUploadId);
        return makeUploadResult();
      },
    );
    const { ref } = renderEditor({ onUploadFile });

    await waitFor(() => expect(ref.current).not.toBeNull());
    ref.current!.uploadFile(new File(["x"], "photo.png", { type: "image/png" }));

    await waitFor(() => expect(seen).toHaveLength(1));
    expect(seen[0]).toBeTruthy();
    expect(typeof seen[0]).toBe("string");
  });

  it("keeps a pending upload out of the markdown, and writes it in once settled", async () => {
    // The draft is the markdown. A pending attachment must not appear in it,
    // and a settled one must — this is the whole reason the plugin registers
    // its own markdown writer instead of reusing LobeHub's.
    let release!: (value: UploadResult) => void;
    const onUploadFile = vi.fn(
      () => new Promise<UploadResult | null>((resolve) => {
        release = (value) => resolve(value);
      }),
    );
    const { ref } = renderEditor({ onUploadFile });

    await waitFor(() => expect(ref.current).not.toBeNull());
    ref.current!.uploadFile(new File(["x"], "photo.png", { type: "image/png" }));

    await waitFor(() => expect(onUploadFile).toHaveBeenCalled());
    expect(ref.current!.getMarkdown()).not.toContain("cdn.example");
    expect(ref.current!.hasActiveUploads()).toBe(true);

    release(makeUploadResult());

    await waitFor(() => {
      expect(ref.current!.getMarkdown()).toContain("https://cdn.example/durable.png");
    });
    expect(ref.current!.hasActiveUploads()).toBe(false);
  });

  it("drops the card when the upload fails, and stops gating", async () => {
    const onUploadFile = vi.fn(() => Promise.reject(new Error("network down")));
    const { ref, container } = renderEditor({ onUploadFile });

    await waitFor(() => expect(ref.current).not.toBeNull());
    ref.current!.uploadFile(new File(["x"], "broken.bin"));

    await waitFor(() => {
      expect(ref.current!.hasActiveUploads()).toBe(false);
    });
    expect(container.querySelector('[data-attachment-status="pending"]')).toBeNull();
    expect(ref.current!.getMarkdown()).not.toContain("broken.bin");
  });

  it("notifies the host when the upload gate changes", async () => {
    const onUploadingChange = vi.fn();
    let release!: (value: UploadResult) => void;
    const onUploadFile = vi.fn(
      () => new Promise<UploadResult | null>((resolve) => {
        release = (value) => resolve(value);
      }),
    );
    const { ref } = renderEditor({ onUploadingChange, onUploadFile });

    await waitFor(() => expect(ref.current).not.toBeNull());
    ref.current!.uploadFile(new File(["x"], "a.bin"));

    await waitFor(() => expect(onUploadingChange).toHaveBeenCalledWith(true));
    release(makeUploadResult());

    await waitFor(() => expect(onUploadingChange).toHaveBeenCalledWith(false));
  });
});
