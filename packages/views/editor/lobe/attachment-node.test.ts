// @vitest-environment node
//
// A headless Lexical editor is enough here: the node's visual is supplied by
// the plugin's decorator, so nothing in this module touches the DOM. That
// keeps the suite cheap and, more usefully, keeps it honest — if a future
// change starts depending on layout, these tests stop being able to run.

import { $getRoot, createEditor } from "lexical";
import { describe, expect, it } from "vitest";

import {
  $createAttachmentNode,
  $isAttachmentNode,
  AttachmentNode,
} from "./attachment-node";

function makeEditor() {
  return createEditor({
    nodes: [AttachmentNode],
    onError: (error) => {
      throw error;
    },
  });
}

/** Runs `fn` inside an update and returns whatever it captured. */
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

describe("AttachmentNode", () => {
  it("starts pending with the id it was created with", () => {
    const editor = makeEditor();
    const node = inEditor(editor, () =>
      $createAttachmentNode({
        clientUploadId: "upload-1",
        filename: "diagram.png",
        fileSize: 2048,
        kind: "image",
      }),
    );

    expect(node.status).toBe("pending");
    expect(node.clientUploadId).toBe("upload-1");
    expect(node.filename).toBe("diagram.png");
    expect(node.fileSize).toBe(2048);
    expect(node.href).toBe("");
  });

  it("promotes to uploaded with the settled href", () => {
    const editor = makeEditor();
    const node = inEditor(editor, () => {
      const created = $createAttachmentNode({
        clientUploadId: "upload-2",
        filename: "spec.pdf",
      });
      created.setUploaded({ href: "https://cdn.example/spec.pdf" });
      return created;
    });

    expect(node.status).toBe("uploaded");
    expect(node.href).toBe("https://cdn.example/spec.pdf");
    expect(node.message).toBe("");
  });

  it("lets setUploaded re-decide the kind", () => {
    // A reopened composer only knew the filename when it drew the placeholder;
    // the server is what actually decides an attachment is an image.
    const editor = makeEditor();
    const node = inEditor(editor, () => {
      const created = $createAttachmentNode({
        clientUploadId: "upload-3",
        filename: "photo",
      });
      expect(created.kind).toBe("file");
      created.setUploaded({ href: "https://cdn.example/photo.png", kind: "image" });
      return created;
    });

    expect(node.kind).toBe("image");
  });

  it("records a failure without losing the id", () => {
    // The id must survive an error: the draft record still points at this node,
    // and a retry settles through the same id.
    const editor = makeEditor();
    const node = inEditor(editor, () => {
      const created = $createAttachmentNode({
        clientUploadId: "upload-4",
        filename: "big.zip",
      });
      created.setError("Payload too large");
      return created;
    });

    expect(node.status).toBe("error");
    expect(node.message).toBe("Payload too large");
    expect(node.clientUploadId).toBe("upload-4");
  });

  it("keeps the clientUploadId across a JSON round trip", () => {
    // THE invariant this node exists for. An upload that outlives its mount
    // settles against a different editor instance showing the same draft, and
    // the id is the only thing that finds the node to settle. If parsing drops
    // it, that write-back silently lands nowhere.
    const source = makeEditor();
    inEditor(source, () => {
      // `append` rather than `$insertNodes`: this editor is headless and has no
      // selection, so an insert has nowhere defined to land.
      $getRoot().append(
        $createAttachmentNode({
          clientUploadId: "survives-the-trip",
          filename: "report.pdf",
          fileSize: 900,
        }),
      );
    });

    const serialized = JSON.stringify(source.getEditorState().toJSON());

    const restored = makeEditor();
    restored.setEditorState(restored.parseEditorState(serialized));

    const node = restored.getEditorState().read(() => {
      const first = $getRoot().getFirstChild();
      return $isAttachmentNode(first) ? first : null;
    });

    expect(node).not.toBeNull();
    expect(node!.clientUploadId).toBe("survives-the-trip");
    expect(node!.filename).toBe("report.pdf");
    expect(node!.fileSize).toBe(900);
    expect(node!.status).toBe("pending");
  });

  it("round-trips a settled node's href", () => {
    const source = makeEditor();
    inEditor(source, () => {
      const node = $createAttachmentNode({
        clientUploadId: "settled",
        filename: "done.png",
        kind: "image",
      });
      node.setUploaded({ href: "https://cdn.example/done.png" });
      $getRoot().append(node);
    });

    const restored = makeEditor();
    restored.setEditorState(
      restored.parseEditorState(JSON.stringify(source.getEditorState().toJSON())),
    );

    const node = restored.getEditorState().read(() => {
      const first = $getRoot().getFirstChild();
      return $isAttachmentNode(first) ? first : null;
    });

    expect(node!.status).toBe("uploaded");
    expect(node!.href).toBe("https://cdn.example/done.png");
    expect(node!.kind).toBe("image");
  });

  it("contributes no text to the document", () => {
    // Markdown output is the registered writer's job, and it is where the
    // pending rule lives. If the node also contributed text, a pending upload
    // would leak into the draft through this path instead.
    const editor = makeEditor();
    const text = inEditor(editor, () => {
      $getRoot().append(
        $createAttachmentNode({
          clientUploadId: "quiet",
          filename: "loud-name.pdf",
        }),
      );
      return $getRoot().getTextContent();
    });

    expect(text).toBe("");
  });
});
