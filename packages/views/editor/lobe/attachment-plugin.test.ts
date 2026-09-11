// @vitest-environment node
//
// Only the markdown writer is exercised here. Node registration and the
// decorator need a kernel, and are covered where the editor is mounted; what
// this file owns is the rule that decides what a *draft* may claim — small,
// pure, and the one thing that must never regress silently.

import { $createParagraphNode, $createTextNode, $getRoot, createEditor } from "lexical";
import { describe, expect, it } from "vitest";

import { $createAttachmentNode, AttachmentNode } from "./attachment-node";
import {
  writeAttachmentMarkdown,
  type MarkdownWriterContext,
} from "./attachment-plugin";

/** Records what the writer emitted, standing in for the real context. */
function makeContext() {
  const lines: string[] = [];
  return {
    lines,
    ctx: {
      appendLine: (line: string) => {
        lines.push(line);
      },
    } as MarkdownWriterContext,
  };
}

function makeEditor() {
  return createEditor({
    nodes: [AttachmentNode],
    onError: (error) => {
      throw error;
    },
  });
}

function inEditor<T>(editor: ReturnType<typeof makeEditor>, fn: () => T): T {
  let captured!: T;
  editor.update(
    () => {
      captured = fn();
    },
    { discrete: true },
  );
  return captured;
}

describe("writeAttachmentMarkdown", () => {
  it("writes nothing for a pending upload", () => {
    // THE invariant. The markdown produced here is what the draft store
    // persists; a draft that mentions an unfinished upload would hand the user
    // back a document claiming a file that may never arrive.
    const editor = makeEditor();
    const node = inEditor(editor, () =>
      $createAttachmentNode({
        clientUploadId: "pending-1",
        filename: "half-sent.pdf",
      }),
    );
    const { ctx, lines } = makeContext();

    writeAttachmentMarkdown(ctx, node);

    expect(lines).toEqual([]);
  });

  it("writes nothing for a failed upload", () => {
    // An errored node is transient: the coordinated-upload path removes it and
    // the draft record carries the failure. It must not reach markdown either.
    const editor = makeEditor();
    const node = inEditor(editor, () => {
      const created = $createAttachmentNode({
        clientUploadId: "failed-1",
        filename: "rejected.zip",
      });
      created.setError("Payload too large");
      return created;
    });
    const { ctx, lines } = makeContext();

    writeAttachmentMarkdown(ctx, node);

    expect(lines).toEqual([]);
  });

  it("writes an image form for a settled image", () => {
    const editor = makeEditor();
    const node = inEditor(editor, () => {
      const created = $createAttachmentNode({
        clientUploadId: "img-1",
        filename: "diagram.png",
        kind: "image",
      });
      created.setUploaded({ href: "https://cdn.example/diagram.png" });
      return created;
    });
    const { ctx, lines } = makeContext();

    writeAttachmentMarkdown(ctx, node);

    expect(lines).toEqual(["![diagram.png](https://cdn.example/diagram.png)"]);
  });

  it("writes a link form for a settled non-image", () => {
    const editor = makeEditor();
    const node = inEditor(editor, () => {
      const created = $createAttachmentNode({
        clientUploadId: "doc-1",
        filename: "spec.pdf",
      });
      created.setUploaded({ href: "https://cdn.example/spec.pdf" });
      return created;
    });
    const { ctx, lines } = makeContext();

    writeAttachmentMarkdown(ctx, node);

    expect(lines).toEqual(["[spec.pdf](https://cdn.example/spec.pdf)"]);
  });

  it("falls back to a generic label when the filename is empty", () => {
    const editor = makeEditor();
    const node = inEditor(editor, () => {
      const created = $createAttachmentNode({
        clientUploadId: "anon-1",
        filename: "",
      });
      created.setUploaded({ href: "https://cdn.example/x" });
      return created;
    });
    const { ctx, lines } = makeContext();

    writeAttachmentMarkdown(ctx, node);

    expect(lines).toEqual(["[attachment](https://cdn.example/x)"]);
  });

  it("ignores nodes that are not attachments", () => {
    const editor = makeEditor();
    const text = inEditor(editor, () => {
      const paragraph = $createParagraphNode();
      const node = $createTextNode("just words");
      paragraph.append(node);
      $getRoot().append(paragraph);
      return node;
    });
    const { ctx, lines } = makeContext();

    writeAttachmentMarkdown(ctx, text);

    expect(lines).toEqual([]);
  });
});
