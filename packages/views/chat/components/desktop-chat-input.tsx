"use client";

import type { CSSProperties, HTMLAttributes, ReactNode, Ref } from "react";
import { Flexbox } from "@lobehub/ui/es/Flex/index";
import { cn } from "@patchbay/ui/lib/utils";
import { CHAT_COLUMN, CHAT_GUTTER } from "./chat-column";

/**
 * Desktop composer chrome mapped from the official LobeHub product repo
 * https://github.com/lobehub/lobehub (`lobehub/lobe-chat` 301s here; same
 * repository, not a fork):
 * `src/features/ChatInput/Desktop/index.tsx`
 * (ChatInput + ChatInputActionBar shell around an editor).
 *
 * LobeHub's npm `@lobehub/editor` ChatInput is a resizable Lexical host. This
 * product already owns Tiptap (slash, mentions, attachments, drafts) and
 * auto-grows the draft up to half the surface (PB-5196). The mapping therefore
 * reuses `@lobehub/ui` Flexbox with the same header / body / footer ActionBar
 * geometry, and keeps ContentEditor as the body. Flexbox layout requires the
 * `.lobe-flex` contract in `@patchbay/ui` `styles/base.css` (this app does not
 * mount LobeHub ThemeProvider on the Agent shell). The column Flexbox is
 * `width="100%"` so that contract does not shrink the composer to its actions.
 */
export type DesktopChatInputProps = {
  composerRef: Ref<HTMLDivElement>;
  dropZoneProps?: HTMLAttributes<HTMLDivElement>;
  uploadEnabled?: boolean;
  noAgent?: boolean;
  header?: ReactNode;
  leftActions: ReactNode;
  rightActions: ReactNode;
  overlay?: ReactNode;
  children: ReactNode;
  actionBarStyle?: CSSProperties;
};

export function DesktopChatInput({
  composerRef,
  dropZoneProps,
  uploadEnabled,
  noAgent,
  header,
  leftActions,
  rightActions,
  overlay,
  children,
  actionBarStyle,
}: DesktopChatInputProps) {
  return (
    <div
      ref={composerRef}
      className={cn(
        // Same 50% cap as the previous composer wrapper — LobeHub's default
        // 64px resizable body would reintroduce the five-line porthole.
        "flex max-h-[50%] min-h-0 flex-col pt-0",
        CHAT_GUTTER,
        "relative z-10",
        noAgent && "cursor-not-allowed",
      )}
    >
      <Flexbox
        className={cn(CHAT_COLUMN, noAgent && "pointer-events-none opacity-60")}
        gap={8}
        paddingBlock="0 8px"
        width="100%"
        aria-disabled={noAgent || undefined}
      >
        <div
          data-slot="chat-input-surface"
          data-lobe-chat-input=""
          data-testid="chat-input"
          {...(uploadEnabled ? dropZoneProps : {})}
          className={cn(
            // Chrome from @lobehub/editor ChatInput `containerLight` /
            // `containerDark` (borderRadiusLG, elevated bg, 4px drop shadow).
            "relative flex min-h-0 max-h-96 flex-col overflow-hidden rounded-xl border border-border bg-background",
            "shadow-[0_4px_4px_color-mix(in_srgb,#000_4%,transparent)]",
            "dark:border-white/10 dark:shadow-[0_4px_4px_color-mix(in_srgb,#000_40%,transparent)]",
          )}
        >
          {header}
          <div className="min-h-9 flex-1 overflow-y-auto px-3 pt-2">{children}</div>
          <Flexbox
            data-slot="chat-input-actions"
            align="center"
            gap={4}
            horizontal
            justify="space-between"
            padding={4}
            style={actionBarStyle ?? { paddingRight: 8 }}
          >
            {leftActions}
            {rightActions}
          </Flexbox>
          {overlay}
        </div>
      </Flexbox>
    </div>
  );
}
