import { fromMarkdown } from "mdast-util-from-markdown";

type PositionedNode = {
  type: string;
  children?: PositionedNode[];
  position?: {
    start?: { offset?: number };
    end?: { offset?: number };
  };
};

export type MarkdownInlineMarker = {
  /** JavaScript string offset in the original Markdown source. */
  offset: number;
  /** Complete Markdown inserted at the resolved safe boundary. */
  markdown: string;
};

const ATOMIC_MARKDOWN_NODES = new Set([
  "code",
  "definition",
  "html",
  "image",
  "imageReference",
  "inlineCode",
  "link",
  "linkReference",
  "yaml",
]);

function safeOffsetInChildren(
  children: PositionedNode[],
  requestedOffset: number,
): number | undefined {
  for (const child of children) {
    const start = child.position?.start?.offset;
    const end = child.position?.end?.offset;
    if (
      start == null ||
      end == null ||
      requestedOffset < start ||
      requestedOffset > end
    ) {
      continue;
    }

    if (child.type === "text") return requestedOffset;
    if (child.children && !ATOMIC_MARKDOWN_NODES.has(child.type)) {
      const nested = safeOffsetInChildren(child.children, requestedOffset);
      if (nested != null) return nested;
    }

    // A citation boundary inside Markdown syntax must not split that construct.
    // Place it after the whole inline-code/link/image/etc. node instead.
    return end;
  }
  return undefined;
}

/**
 * Inserts trusted marker Markdown without splitting an existing Markdown node.
 * Offsets remain relative to the original source; insertions are applied from
 * right to left so an earlier insertion cannot move a later boundary.
 */
export function insertMarkdownInlineMarkers(
  content: string,
  markers: MarkdownInlineMarker[],
): string {
  if (markers.length === 0) return content;

  let tree: PositionedNode;
  try {
    tree = fromMarkdown(content) as PositionedNode;
  } catch {
    return content;
  }

  const grouped = new Map<number, string[]>();
  for (const marker of markers) {
    if (
      !Number.isInteger(marker.offset) ||
      marker.offset < 0 ||
      marker.offset > content.length ||
      marker.markdown.length === 0
    ) {
      continue;
    }
    const safeOffset = tree.children
      ? (safeOffsetInChildren(tree.children, marker.offset) ?? marker.offset)
      : marker.offset;
    grouped.set(safeOffset, [
      ...(grouped.get(safeOffset) ?? []),
      marker.markdown,
    ]);
  }

  let result = content;
  for (const [offset, markdown] of [...grouped].sort(([a], [b]) => b - a)) {
    result = `${result.slice(0, offset)}${markdown.join("")}${result.slice(offset)}`;
  }
  return result;
}
