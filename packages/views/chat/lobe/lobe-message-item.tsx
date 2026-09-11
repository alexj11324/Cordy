"use client";

/**
 * A chat message rendered by LobeHub's `ChatItem`.
 *
 * This replaces the `Message` / `MessageContent` pair from AI Elements, which
 * were unopinionated Tailwind divs: no avatar, no title, and an actions bar the
 * caller had to reveal on hover by hand. `ChatItem` supplies all of that —
 * including the detail that the actions bar must stay visible while one of its
 * dropdowns is open, which is easy to get wrong when hand-rolling
 * `group-hover`.
 *
 * Deliberately presentational: it takes a role and children, not a
 * `ChatMessage`. The message shape is core's, and keeping the mapping out of
 * here means this component has no reason to care when the API drifts.
 *
 * Must be rendered inside `LobeThemeBridge` — that is what gives the antd
 * layer its tokens.
 */

import { ChatItem } from "@lobehub/ui/chat";
import type { ReactNode } from "react";

/** Who authored the message. Anything that is not `user` reads as the agent. */
export type LobeMessageRole = "user" | "assistant";

/** Avatar metadata, mirroring `@lobehub/ui`'s `MetaData`. */
export interface LobeMessageAvatar {
  /** Image URL, or a node such as an initials badge. */
  avatar?: string | ReactNode;
  title?: string;
}

export interface LobeMessageItemProps {
  /** Rendered inside the bubble. */
  children: ReactNode;
  /** Hover-revealed action row beneath the bubble. */
  actions?: ReactNode;
  /** Always-visible content below the bubble (citations, usage, follow-ups). */
  belowMessage?: ReactNode;
  /** Author metadata; defaults to a role-appropriate placeholder. */
  avatar?: LobeMessageAvatar;
  /** Shown while the reply is still arriving. */
  loading?: boolean;
  role: LobeMessageRole;
  /** Epoch milliseconds. */
  time?: number;
}

/**
 * User messages sit on the right, everything else on the left.
 *
 * Exported so the mapping can be asserted directly. Reading it back out of
 * rendered markup would mean parsing antd's generated class names, and each
 * render of the theme bridge costs seconds in jsdom.
 */
export function toChatItemPlacement(role: LobeMessageRole): "left" | "right" {
  return role === "user" ? "right" : "left";
}

export function LobeMessageItem({
  actions,
  avatar,
  belowMessage,
  children,
  loading,
  role,
  time,
}: LobeMessageItemProps) {
  const fallbackTitle = role === "user" ? "You" : "Agent";

  return (
    <ChatItem
      actions={actions}
      avatar={{
        avatar: avatar?.avatar,
        title: avatar?.title ?? fallbackTitle,
      }}
      belowMessage={belowMessage}
      loading={loading}
      placement={toChatItemPlacement(role)}
      // `ChatItem` does not render arbitrary children — it `Omit`s them from
      // its props. The `message` prop looks like the way in, but internally it
      // is coerced with `String()` and handed to a markdown editor, so passing
      // a ReactNode there renders "[object Object]". `renderMessage` is the
      // supported escape hatch: its return value replaces that editor node
      // outright, which is what lets our own rich content through intact.
      renderMessage={() => children}
      showAvatar
      showTitle
      time={time}
      variant="bubble"
    />
  );
}
