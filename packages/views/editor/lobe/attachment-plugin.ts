/**
 * Kernel plugin for the attachment node.
 *
 * Mirrors the shape of LobeHub's own `FilePlugin` (register the node, register
 * a decorator, register a markdown writer) without extending its internal
 * `KernelPlugin` base — that class ships no `.d.ts`, so it is not part of the
 * package's consumable surface. `IEditorPluginConstructor` only asks for
 * `{ pluginName, new(kernel, config) }`, which a plain class satisfies.
 *
 * The one place this deliberately diverges from `FilePlugin` is the markdown
 * writer, and it is the reason this plugin exists at all — see
 * {@link registerAttachmentMarkdownWriter}.
 */

import { IMarkdownShortCutService, type IEditor } from "@lobehub/editor";
import type { LexicalEditor, LexicalNode } from "lexical";

import { AttachmentNode, $isAttachmentNode } from "./attachment-node";
import { escapeMarkdownLabel } from "../utils/escape-markdown-label";

/*
 * The kernel-side types this plugin is built from — `IEditorKernel`,
 * `IEditorPlugin`, `IEditorPluginConstructor`, `IDecorator`,
 * `IMarkdownWriterContext` — are deliberately NOT exported by the package.
 * Its `exports` map has no wildcard, so the usual escape hatch of deep
 * importing `es/types/kernel.js` is closed too.
 *
 * They are all reachable from `IEditor`, which IS public, so they are derived
 * here rather than re-declared. Deriving keeps them in lockstep with the
 * library: a signature change surfaces as a type error in this file instead of
 * as a silently wrong structural copy.
 */
type PluginConstructor = Parameters<IEditor["registerPlugin"]>[0];
/** The kernel handle a plugin receives in its constructor. */
type PluginKernel = ConstructorParameters<PluginConstructor>[0];
type Decorator = Parameters<PluginKernel["registerDecorator"]>[1];
/**
 * Derived from the markdown service, which *is* exported, rather than from the
 * kernel — `requireService` is generic, so reading the writer's signature back
 * out of it collapses to `unknown`.
 */
export type MarkdownWriterContext = Parameters<
  Parameters<IMarkdownShortCutService["registerMarkdownWriter"]>[1]
>[0];

/** What the host supplies as the settled location of an upload. */
export interface AttachmentSettleResult {
  /** Durable URL to write into markdown. */
  href: string;
  /** Re-decided by the server when it disagrees with the filename. */
  kind?: "file" | "image";
}

export interface AttachmentPluginOptions {
  /** Renders the node. Supplied by the React wrapper. */
  decorator?: (node: AttachmentNode, editor: LexicalEditor) => unknown;
}

/**
 * Writes the markdown for one attachment node.
 *
 * A `pending` or `error` node writes **nothing**. This is the invariant the
 * whole upload design rests on: the markdown this produces is what the draft
 * store persists, and a draft must never claim an attachment that has not
 * finished. Where LobeHub's `FilePlugin` emits `Uploading ${name}...`, we emit
 * nothing, matching the TipTap editor's existing behaviour
 * (`packages/views/editor/extensions/index.ts`, which returns `""` for a node
 * whose `uploading` attribute is set).
 *
 * A settled node writes the ordinary markdown for its kind, so a reopened
 * draft round-trips to the same document the user saw before.
 */
export function writeAttachmentMarkdown(
  ctx: MarkdownWriterContext,
  node: LexicalNode,
): void {
  if (!$isAttachmentNode(node)) return;
  if (node.status !== "uploaded") return;

  // Escaped, never raw: the filename is user-controlled, and a `]` in it ends
  // the label. `[report[final].pdf](…)` parses back as the label
  // `report[final]` followed by literal text, which destroys the link and
  // corrupts the draft on reload. Both TipTap writers escape the same way
  // (`extensions/index.ts` for the image alt, `extensions/file-card.tsx` for
  // the card), through this same helper — one rule, one implementation.
  const label = escapeMarkdownLabel(node.filename || "attachment");
  // Images use the image form so a reload renders a picture; everything else
  // is a link. The href is the durable URL the host settled with.
  ctx.appendLine(
    node.kind === "image" ? `![${label}](${node.href})` : `[${label}](${node.href})`,
  );
}

export class AttachmentPlugin {
  static readonly pluginName = "AttachmentPlugin";

  private readonly kernel: PluginKernel;
  private readonly config: AttachmentPluginOptions | undefined;

  constructor(kernel: PluginKernel, config?: AttachmentPluginOptions) {
    this.kernel = kernel;
    this.config = config;

    kernel.registerNodes([AttachmentNode]);
    kernel.registerDecorator(
      AttachmentNode.getType(),
      // Tolerates a missing decorator: the node still round-trips through
      // markdown and JSON, it just renders nothing. That keeps headless and
      // markdown-only consumers working without a React tree.
      ((node: LexicalNode, editor: LexicalEditor) =>
        this.config?.decorator && $isAttachmentNode(node)
          ? this.config.decorator(node, editor)
          : null) as Decorator,
    );
  }

  onInit(_editor: LexicalEditor): void {
    registerAttachmentMarkdownWriter(this.kernel);
  }

  destroy(): void {
    // Nothing to tear down: the kernel owns node, decorator and service
    // registries and drops them with the editor.
  }
}

/** Idempotent: the kernel keeps one writer per node type. */
function registerAttachmentMarkdownWriter(kernel: PluginKernel): void {
  const markdown = kernel.requireService(IMarkdownShortCutService);
  if (!markdown) return;
  markdown.registerMarkdownWriter(
    AttachmentNode.getType(),
    (ctx, node) => writeAttachmentMarkdown(ctx, node),
  );
}

/**
 * The registration-facing shape of the plugin.
 *
 * Written out by hand rather than taken from {@link PluginConstructor}, whose
 * `Parameters<>` derivation instantiates `registerPlugin`'s generic with its
 * constraint — so the config type collapses to `unknown` and every decorator
 * parameter becomes an implicit `any` at the call site. Spelling the signature
 * out keeps `AttachmentPluginOptions` visible, so `registerPlugin` infers it.
 *
 * The cast inside is for `pluginName`: `IEditorPluginConstructor` declares it
 * instance-visible while the class carries it as a static — which is how every
 * plugin in the library itself is written.
 */
export interface AttachmentPluginConstructor {
  new (
    kernel: PluginKernel,
    config?: AttachmentPluginOptions,
  ): {
    destroy(): void;
    onInit?(editor: LexicalEditor): void;
  };
  readonly pluginName: string;
}

export const attachmentPlugin: AttachmentPluginConstructor =
  AttachmentPlugin as unknown as AttachmentPluginConstructor;

/** Convenience for callers that only need the editor's plugin registration. */
export function registerAttachmentPlugin(
  editor: IEditor,
  options?: AttachmentPluginOptions,
): void {
  editor.registerPlugin(attachmentPlugin, options);
}
