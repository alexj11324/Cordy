import type { ChatMessage, MessageSource } from "@orvilo/core/types";
import { insertMarkdownInlineMarkers } from "@orvilo/core/markdown";

export interface ResolvedMobileCitation {
  source: MessageSource;
  sourceNumber: number;
  end: number;
}

export function validMessageSources(message: ChatMessage): MessageSource[] {
  return (message.sources ?? []).filter((source) => {
    try {
      const url = new URL(source.url);
      return url.protocol === "https:" || url.protocol === "http:";
    } catch {
      return false;
    }
  });
}

export function contentWithInlineCitations(message: ChatMessage): string {
  const sources = validMessageSources(message);
  const byId = new Map(
    sources.map((source, index) => [source.id, { source, index }]),
  );
  const bytes = new TextEncoder().encode(message.content);
  const citations: ResolvedMobileCitation[] = [];
  for (const citation of message.citations ?? []) {
    const resolved = byId.get(citation.source_id);
    if (
      !resolved ||
      !Number.isInteger(citation.start) ||
      !Number.isInteger(citation.end) ||
      citation.start < 0 ||
      citation.end <= citation.start ||
      citation.end > bytes.length
    ) {
      continue;
    }
    citations.push({
      source: resolved.source,
      sourceNumber: resolved.index + 1,
      end: citation.end,
    });
  }
  if (citations.length === 0) return message.content;

  const decoder = new TextDecoder("utf-8", { fatal: false });
  return insertMarkdownInlineMarkers(
    message.content,
    citations.map(({ source, sourceNumber, end }) => ({
      offset: decoder.decode(bytes.slice(0, end)).length,
      markdown: ` [${sourceNumber}](<${source.url}>)`,
    })),
  );
}
