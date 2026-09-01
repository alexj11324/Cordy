import {
  detectLinks,
  findCodeRanges,
  findMarkdownLinkRanges,
  isInsideCode,
  rangesOverlap,
} from "@patchbay/ui/markdown/linkify";
import type { ResolvedIssueRef } from "../extensions/issue-identifier-autolink";

const IDENTIFIER_RE = /(?<![A-Za-z0-9_-])([A-Z][A-Z0-9]*-\d+)(?![A-Za-z0-9_-])/g;

export type BareIssueIdentifierRange = {
  identifier: string;
  start: number;
  end: number;
};

/**
 * Bare issue identifiers in markdown that are safe to autolink — same skip
 * rules as `preprocessIssueIdentifiers` (code, existing links, URLs, paths).
 */
export function listBareIssueIdentifierRanges(
  text: string,
): BareIssueIdentifierRange[] {
  if (!/[A-Z][A-Z0-9]*-\d/.test(text)) return [];

  const codeRanges = findCodeRanges(text);
  const linkRanges = findMarkdownLinkRanges(text);
  const detectedLinks = detectLinks(text);
  const out: BareIssueIdentifierRange[] = [];

  IDENTIFIER_RE.lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = IDENTIFIER_RE.exec(text)) !== null) {
    const identifier = match[1];
    if (!identifier) continue;
    const start = match.index;
    const end = start + identifier.length;
    const range = { start, end };

    if (isInsideCode(start, codeRanges)) continue;
    if (linkRanges.some((linkRange) => rangesOverlap(range, linkRange))) continue;
    if (detectedLinks.some((link) => rangesOverlap(range, link))) continue;

    const after = text[end];
    if (after === "." && /[A-Za-z0-9]/.test(text[end + 1] ?? "")) continue;
    if (after === "/" || text[start - 1] === "/") continue;
    if (text[start - 1] === ".") continue;

    out.push({ identifier, start, end });
  }
  return out;
}

export async function resolveBareIssueIdentifiersInMarkdown(
  markdown: string,
  resolve: (identifier: string) => Promise<ResolvedIssueRef | null>,
): Promise<string> {
  const ranges = listBareIssueIdentifierRanges(markdown);
  if (ranges.length === 0) return markdown;

  const unique = [...new Set(ranges.map((range) => range.identifier))];
  const resolved = new Map<string, ResolvedIssueRef>();
  await Promise.all(
    unique.map(async (identifier) => {
      try {
        const hit = await resolve(identifier);
        if (hit) resolved.set(identifier, hit);
      } catch {
        // Misses stay plain text, matching Tiptap autolink.
      }
    }),
  );
  if (resolved.size === 0) return markdown;

  let result = markdown;
  for (const range of [...ranges].reverse()) {
    const hit = resolved.get(range.identifier);
    if (!hit) continue;
    const token = `[${hit.identifier}](mention://issue/${hit.id})`;
    result = `${result.slice(0, range.start)}${token}${result.slice(range.end)}`;
  }
  return result;
}
