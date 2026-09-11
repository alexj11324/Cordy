export { LobeThemeBridge } from "../../chat/lobe/lobe-theme-bridge";

export {
  $createAttachmentNode,
  $isAttachmentNode,
  AttachmentNode,
  type AttachmentKind,
  type AttachmentNodePayload,
  type AttachmentStatus,
  type SerializedAttachmentNode,
} from "./attachment-node";

export {
  appendAttachment,
  failAttachment,
  findAttachment,
  hasActiveUploads,
  insertAttachmentPlaceholder,
  insertMarkdownAtEnd,
  listAttachments,
  removeAttachment,
  settleAttachment,
} from "./attachment-ops";

export {
  AttachmentPlugin,
  attachmentPlugin,
  registerAttachmentPlugin,
  writeAttachmentMarkdown,
  type AttachmentPluginConstructor,
  type AttachmentPluginOptions,
  type AttachmentSettleResult,
  type MarkdownWriterContext,
} from "./attachment-plugin";

export {
  LobeContentEditor,
  type LobeContentEditorHandle,
  type LobeContentEditorProps,
} from "./lobe-content-editor";

export { ReactAttachmentPlugin } from "./react-attachment-plugin";

export { toSettleResult, type UploadResultLike } from "./upload-result";
