import { escapeMarkdownLabel } from "../../editor/utils/escape-markdown-label";
import { formatSlashCommandLabel } from "../../editor/extensions/slash-command-utils";

/**
 * Patchbay mention / slash markdown for the Agent Lexical composer.
 *
 * LobeHub's default mention writer emits `<mention />` XML. Agent messages and
 * drafts still use the existing Tiptap tokens:
 *   `[@label](mention://type/id)`
 *   `[/label](slash://skill/id)`
 */
export function mentionChipLabel(type: string, label: string): string {
  if (type === "skill") return `/${formatSlashCommandLabel(label)}`;
  if (type === "issue" || type === "project") return label;
  return `@${label}`;
}

export function serializeComposerMention(mention: {
  label: string;
  metadata?: Record<string, unknown> | null;
}): string {
  const metadata = mention.metadata ?? {};
  const type = typeof metadata.type === "string" ? metadata.type : "member";
  const id = typeof metadata.id === "string" ? metadata.id : "";
  const rawLabel =
    typeof metadata.label === "string" && metadata.label.length > 0
      ? metadata.label
      : stripChipPrefix(mention.label);
  if (type === "skill") {
    const safeLabel = escapeMarkdownLabel(formatSlashCommandLabel(rawLabel));
    return `[/${safeLabel}](slash://skill/${id})`;
  }
  const prefix = type === "issue" || type === "project" ? "" : "@";
  return `[${prefix}${escapeMarkdownLabel(rawLabel)}](mention://${type}/${id})`;
}

function stripChipPrefix(label: string): string {
  return label.replace(/^[@/]/, "");
}
