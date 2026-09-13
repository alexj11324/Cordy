/**
 * The attachment node for the Lexical-backed editor.
 *
 * This is a deliberate re-implementation rather than a reuse of LobeHub's
 * `FileNode`, even though the two look alike. `FileNode` carries
 * `pending | uploaded | error` too — but it has no place for
 * `clientUploadId`, and that id is not an implementation detail: it is the
 * only link between a placeholder drawn in the document and the upload record
 * the draft store persists (MUL-5181). An upload outlives the mount that
 * started it, and when it settles the editor showing the draft is a *different
 * instance* that never owned the promise. Finding the node to settle is done
 * by that id and nothing else.
 *
 * Encoding that link into a third party's node would make a core invariant of
 * this product depend on someone else's data model. Registering our own node
 * keeps the definition here. LobeHub still supplies the host — the kernel,
 * the data sources, the command bus — via `registerNodes`.
 *
 * Kept free of React on purpose: the visual is supplied by the plugin's
 * decorator, so this module can be exercised through a headless Lexical editor
 * with no DOM.
 */

import {
  $applyNodeReplacement,
  DecoratorNode,
  type EditorConfig,
  type LexicalEditor,
  type LexicalNode,
  type LexicalUpdateJSON,
  type SerializedLexicalNode,
  type Spread,
} from "lexical";

/** Where an attachment is in its lifecycle. */
export type AttachmentStatus = "pending" | "uploaded" | "error";

/**
 * Images get an inline preview; everything else renders as a file card. One
 * node type with a discriminator rather than two types, because every rule
 * that applies to one — the upload id, the status, the markdown writer's
 * pending behaviour — applies identically to the other.
 */
export type AttachmentKind = "file" | "image";

export type SerializedAttachmentNode = Spread<
  {
    /** Editor-minted id, adopted by the host as the draft's `clientUploadId`. */
    clientUploadId: string;
    filename: string;
    fileSize: number;
    /** Durable URL once uploaded; empty while pending or on error. */
    href: string;
    kind: AttachmentKind;
    /** Failure text, shown on the card when `status === "error"`. */
    message: string;
    /** Blob URL for the local preview while an image uploads. */
    previewSrc: string;
    status: AttachmentStatus;
  },
  SerializedLexicalNode
>;

export interface AttachmentNodePayload {
  clientUploadId: string;
  filename: string;
  fileSize?: number;
  href?: string;
  kind?: AttachmentKind;
  message?: string;
  previewSrc?: string;
  status?: AttachmentStatus;
}

export class AttachmentNode extends DecoratorNode<null> {
  __clientUploadId: string;
  __filename: string;
  __fileSize: number;
  __href: string;
  __kind: AttachmentKind;
  __message: string;
  __previewSrc: string;
  __status: AttachmentStatus;

  static getType(): string {
    return "attachment";
  }

  static clone(node: AttachmentNode): AttachmentNode {
    return new AttachmentNode(
      {
        clientUploadId: node.__clientUploadId,
        filename: node.__filename,
        fileSize: node.__fileSize,
        href: node.__href,
        kind: node.__kind,
        message: node.__message,
        previewSrc: node.__previewSrc,
        status: node.__status,
      },
      node.__key,
    );
  }

  /**
   * Status is serialized even though the markdown writer hides pending nodes.
   * The two are not in conflict: `exportJSON` exists so undo/redo and
   * `setDocument("json", …)` round-trips keep the node's real state, while
   * markdown is the *draft* representation, and a draft must never claim an
   * upload that has not finished.
   */
  static importJSON(serialized: SerializedAttachmentNode): AttachmentNode {
    return $createAttachmentNode(serialized).updateFromJSON(serialized);
  }

  constructor(payload: AttachmentNodePayload, key?: string) {
    super(key);
    this.__clientUploadId = payload.clientUploadId;
    this.__filename = payload.filename;
    this.__fileSize = payload.fileSize ?? 0;
    this.__href = payload.href ?? "";
    this.__kind = payload.kind ?? "file";
    this.__message = payload.message ?? "";
    this.__previewSrc = payload.previewSrc ?? "";
    this.__status = payload.status ?? "pending";
  }

  updateFromJSON(
    serialized: LexicalUpdateJSON<SerializedAttachmentNode>,
  ): this {
    return super
      .updateFromJSON(serialized)
      .setClientUploadId(serialized.clientUploadId)
      .setFilename(serialized.filename)
      .setFileSize(serialized.fileSize)
      .setHref(serialized.href)
      .setKind(serialized.kind)
      .setMessage(serialized.message)
      .setPreviewSrc(serialized.previewSrc)
      .setStatus(serialized.status);
  }

  exportJSON(): SerializedAttachmentNode {
    return {
      ...super.exportJSON(),
      clientUploadId: this.__clientUploadId,
      filename: this.__filename,
      fileSize: this.__fileSize,
      href: this.__href,
      kind: this.__kind,
      message: this.__message,
      previewSrc: this.__previewSrc,
      status: this.__status,
    };
  }

  createDOM(_config: EditorConfig): HTMLElement {
    // A decorator's DOM is owned by the decorator, not by createDOM.
    return document.createElement("div");
  }

  updateDOM(): false {
    // Every attribute change is a re-decoration, never a DOM patch.
    return false;
  }

  getTextContent(): string {
    // Contributes nothing to the document's text. Markdown output is produced
    // by the registered writer, which is where the pending rule lives.
    return "";
  }

  decorate(_editor: LexicalEditor, _config: EditorConfig): null {
    // Rendered by the plugin's registered decorator so this module stays free
    // of React and can run headless in tests.
    return null;
  }

  get clientUploadId(): string {
    return this.__clientUploadId;
  }

  get filename(): string {
    return this.__filename;
  }

  get fileSize(): number {
    return this.__fileSize;
  }

  get href(): string {
    return this.__href;
  }

  get kind(): AttachmentKind {
    return this.__kind;
  }

  get message(): string {
    return this.__message;
  }

  get previewSrc(): string {
    return this.__previewSrc;
  }

  get status(): AttachmentStatus {
    return this.__status;
  }

  setClientUploadId(id: string): this {
    const writable = this.getWritable();
    writable.__clientUploadId = id;
    return writable;
  }

  setFilename(filename: string): this {
    const writable = this.getWritable();
    writable.__filename = filename;
    return writable;
  }

  setFileSize(size: number): this {
    const writable = this.getWritable();
    writable.__fileSize = size;
    return writable;
  }

  setHref(href: string): this {
    const writable = this.getWritable();
    writable.__href = href;
    return writable;
  }

  setKind(kind: AttachmentKind): this {
    const writable = this.getWritable();
    writable.__kind = kind;
    return writable;
  }

  setMessage(message: string): this {
    const writable = this.getWritable();
    writable.__message = message;
    return writable;
  }

  setPreviewSrc(src: string): this {
    const writable = this.getWritable();
    writable.__previewSrc = src;
    return writable;
  }

  setStatus(status: AttachmentStatus): this {
    const writable = this.getWritable();
    writable.__status = status;
    return writable;
  }

  /**
   * Promote a pending node to its finished form.
   *
   * `kind` is re-decided here because the server, not the file extension,
   * decides what an attachment actually is — and a reopened composer only knew
   * the filename when it drew the placeholder.
   */
  setUploaded(result: { href: string; kind?: AttachmentKind }): this {
    return this.setStatus("uploaded")
      .setHref(result.href)
      .setMessage("")
      .setKind(result.kind ?? this.__kind);
  }

  setError(message: string): this {
    return this.setStatus("error").setMessage(message);
  }
}

export function $createAttachmentNode(
  payload: AttachmentNodePayload,
): AttachmentNode {
  return $applyNodeReplacement(new AttachmentNode(payload));
}

export function $isAttachmentNode(
  node: LexicalNode | null | undefined,
): node is AttachmentNode {
  return node instanceof AttachmentNode;
}
