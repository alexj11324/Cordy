"use client"

import {
  Frame,
  FrameHeader,
  FramePanel,
  FrameTitle,
} from "@orvilo/ui/components/reui/frame"

import { Button } from "@orvilo/ui/components/ui/button"
import { Progress } from "@orvilo/ui/components/ui/progress"
import { SidebarGroup } from "@orvilo/ui/components/ui/sidebar"
import { SparklesIcon } from "lucide-react"

// ── Upgrade Card ──

const PLAN = {
  used: 8240,
  total: 10000,
  label: "AI Credits",
  resetLabel: "Usage resets at the end of your billing cycle.",
} as const

export function UpgradeCard() {
  const pct = Math.round((PLAN.used / PLAN.total) * 100)
  const freePct = 100 - pct

  return (
    <SidebarGroup className="overflow-hidden group-data-[collapsible=icon]:hidden">
      <Frame
        spacing="sm"
        dense
        className="mx-auto shrink-0 overflow-hidden border border-zinc-700 bg-zinc-900/90 md:w-[220px]"
      >
        <FrameHeader className="bg-zinc-900/90">
          <div className="flex items-center gap-1.5">
            <span className="size-3.5 text-yellow-500 [&_svg]:size-3.5">
              <SparklesIcon aria-hidden="true" />
            </span>
            <FrameTitle className="text-xs text-yellow-500">
              {PLAN.label}
            </FrameTitle>
          </div>
        </FrameHeader>

        <FramePanel className="space-y-2.5 border-zinc-700 bg-zinc-900/80">
          <p className="text-xs leading-snug text-zinc-400">
            {PLAN.resetLabel}
          </p>

          <div className="bg-muted/55 relative h-1.5 overflow-hidden rounded-sm">
            <div
              className="pointer-events-none absolute inset-0 text-white opacity-20"
              aria-hidden="true"
              style={{
                backgroundImage:
                  "repeating-linear-gradient(-45deg, currentColor 0, currentColor 1px, transparent 0, transparent 4px)",
              }}
            />
            <Progress
              value={pct}
              className="absolute inset-0 gap-0 **:data-[slot=progress-indicator]:rounded-none **:data-[slot=progress-indicator]:bg-emerald-500 **:data-[slot=progress-track]:h-full **:data-[slot=progress-track]:rounded-none **:data-[slot=progress-track]:bg-transparent"
            />
          </div>

          <div className="flex items-center justify-between text-xs leading-none">
            <div className="flex items-center gap-1">
              <span className="font-semibold text-zinc-100 tabular-nums">
                {pct}%
              </span>
              <span className="text-zinc-400">Used</span>
            </div>
            <div className="flex items-center gap-1">
              <span className="font-semibold text-zinc-100 tabular-nums">
                {freePct}%
              </span>
              <span className="text-zinc-400">Free</span>
            </div>
          </div>

          <Button variant="outline" size="sm" className="dark w-full">
            Upgrade Plan
          </Button>
        </FramePanel>
      </Frame>
    </SidebarGroup>
  )
}