import type { ChatMessage, MessageSource } from "@orvilo/core/types";

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

  const byEnd = new Map<number, ResolvedMobileCitation[]>();
  for (const citation of citations) {
    byEnd.set(citation.end, [...(byEnd.get(citation.end) ?? []), citation]);
  }
  const decoder = new TextDecoder("utf-8", { fatal: false });
  let cursor = 0;
  let result = "";
  for (const [end, endingCitations] of [...byEnd].sort(([a], [b]) => a - b)) {
    if (end <= cursor) continue;
    result += decoder.decode(bytes.slice(cursor, end));
    result += endingCitations
      .map(({ source, sourceNumber }) => ` [${sourceNumber}](<${source.url}>)`)
      .join("");
    cursor = end;
  }
  return result + decoder.decode(bytes.slice(cursor));
}
