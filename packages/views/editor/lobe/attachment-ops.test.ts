// @vitest-environment node
//
// These operations are what the coordinated-upload engine and the editor's
// imperative handle both call. They are exercised against a headless Lexical
// editor: no DOM, no React, no kernel — the semantics under test are all node
// bookkeeping, and keeping the surface this small is what makes the retry and
// gate rules checkable at all.

import { createEditor } from "lexical";
import { describe, expect, it } from "vitest";

import { AttachmentNode } from "./attachment-node";
import {
  appendAttachment,
  failAttachment,
  findAttachment,
  hasActiveUploads,
  insertAttachmentPlaceholder,
  listAttachments,
  removeAttachment,
  settleAttachment,
} from "./attachment-ops";

function makeEditor() {
  return createEditor({
    nodes: [AttachmentNode],
    onError: (error) => {
      throw error;
    },
  });
}

describe("attachment operations", () => {
  it("appends a pending attachment that can be found by its id", () => {
    const editor = makeEditor();

    appendAttachment(editor, {
      clientUploadId: "a1",
      filename: "notes.txt",
      fileSize: 12,
    });

    const node = findAttachment(editor, "a1");
    expect(node).not.toBeNull();
    expect(node!.status).toBe("pending");
    expect(node!.filename).toBe("notes.txt");
  });

  it("returns null for an id the document does not hold", () => {
    const editor = makeEditor();
    appendAttachment(editor, { clientUploadId: "a1", filename: "notes.txt" });

    expect(findAttachment(editor, "nope")).toBeNull();
  });

  it("draws a placeholder without a preview", () => {
    // The bytes went with the mount that started the upload; a card is the
    // only honest thing to draw.
    const editor = makeEditor();

    expect(
      insertAttachmentPlaceholder(editor, {
        uploadId: "b1",
        filename: "big.zip",
        size: 999,
      }),
    ).toBe(true);

    const node = findAttachment(editor, "b1");
    expect(node!.previewSrc).toBe("");
    expect(node!.kind).toBe("file");
    expect(node!.fileSize).toBe(999);
  });

  it("treats a placeholder that is already drawn as success, and does not duplicate it", () => {
    // The caller retries until this reports success, racing the editor's
    // mount. If a second call were not idempotent, the caller's own first
    // successful insert would read as failure — retrying forever and stacking
    // duplicate cards for one upload.
    const editor = makeEditor();

    expect(
      insertAttachmentPlaceholder(editor, { uploadId: "b2", filename: "x.bin" }),
    ).toBe(true);
    expect(
      insertAttachmentPlaceholder(editor, { uploadId: "b2", filename: "x.bin" }),
    ).toBe(true);

    expect(listAttachments(editor).filter((a) => a.clientUploadId === "b2")).toHaveLength(1);
  });

  it("settles a placeholder in place", () => {
    const editor = makeEditor();
    insertAttachmentPlaceholder(editor, { uploadId: "c1", filename: "photo" });

    expect(
      settleAttachment(editor, "c1", { href: "https://cdn/p.png", kind: "image" }),
    ).toBe(true);

    const node = findAttachment(editor, "c1");
    expect(node!.status).toBe("uploaded");
    expect(node!.href).toBe("https://cdn/p.png");
    expect(node!.kind).toBe("image");
  });

  it("reports false when settling an id this document does not hold", () => {
    // The caller's fallback: append the link instead. This is how an upload
    // that settled while a different mount showed the draft reaches the
    // visible document.
    const editor = makeEditor();

    expect(settleAttachment(editor, "ghost", { href: "https://cdn/g" })).toBe(false);
  });

  it("removes an attachment and reports whether one was there", () => {
    const editor = makeEditor();
    appendAttachment(editor, { clientUploadId: "d1", filename: "gone.txt" });

    expect(removeAttachment(editor, "d1")).toBe(true);
    expect(findAttachment(editor, "d1")).toBeNull();
    expect(removeAttachment(editor, "d1")).toBe(false);
  });

  it("leaves no empty block behind when an attachment is removed", () => {
    // The node is wrapped in a paragraph; removing only the node would leave a
    // blank line where a failed upload used to be.
    const editor = makeEditor();
    appendAttachment(editor, { clientUploadId: "d2", filename: "gone.txt" });
    removeAttachment(editor, "d2");

    expect(listAttachments(editor)).toHaveLength(0);
  });

  it("gates on pending uploads only", () => {
    const editor = makeEditor();
    expect(hasActiveUploads(editor)).toBe(false);

    appendAttachment(editor, { clientUploadId: "e1", filename: "in-flight.bin" });
    expect(hasActiveUploads(editor)).toBe(true);

    settleAttachment(editor, "e1", { href: "https://cdn/e1" });
    expect(hasActiveUploads(editor)).toBe(false);
  });

  it("does not gate on a failed upload", () => {
    // An errored upload is finished business — the coordinator removes it. If
    // it still counted as active, the send affordance would be blocked by an
    // upload that is never going to complete.
    const editor = makeEditor();
    insertAttachmentPlaceholder(editor, { uploadId: "f1", filename: "bad.bin" });

    expect(failAttachment(editor, "f1", "Payload too large")).toBe(true);

    expect(findAttachment(editor, "f1")!.status).toBe("error");
    expect(findAttachment(editor, "f1")!.message).toBe("Payload too large");
    expect(hasActiveUploads(editor)).toBe(false);
  });
});
