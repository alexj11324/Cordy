/**
 * Bridges the host's upload result to what the attachment node stores.
 *
 * The two shapes are close but not the same, and the difference is the whole
 * point of this module: an `UploadResult` carries several URLs (the raw
 * storage URL, the durable download path, and the markdown-formatted link),
 * while the node stores exactly one — whichever belongs in the document.
 *
 * That choice is already made upstream by `pickMarkdownLink` in
 * `@orvilo/core/hooks/use-file-upload`, which prefers the server-provided
 * durable URL and falls back to the legacy site-relative path. Re-deciding it
 * here would fork that policy; this reads `markdownLink` and nothing else.
 */

import type { UploadResult } from "@orvilo/core/hooks/use-file-upload";

import type { AttachmentSettleResult } from "./attachment-plugin";

/** What a host's `onUploadFile` resolves with: the app's own upload result. */
export type UploadResultLike = UploadResult;

/**
 * Map a completed upload into the node's settled form.
 *
 * `kind` is left unset unless the caller knows better: the editor already
 * decided image-vs-file from the MIME type when it drew the node, and
 * `settleAttachment` only re-decides when a value is present.
 */
export function toSettleResult(
  result: UploadResultLike,
  kind?: "file" | "image",
): AttachmentSettleResult {
  return { href: result.markdownLink, kind };
}
