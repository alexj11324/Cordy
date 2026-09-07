"use client";

import { useMemo, useState, type ReactNode } from "react";
import { Clock, Plus, Search, Webhook } from "lucide-react";
import {
  AUTOMATION_TRIGGER_PRESETS,
  AUTOMATION_TRIGGER_SOURCES,
  presetsForSource,
  searchTriggerCatalog,
  type AutomationTriggerPreset,
  type AutomationTriggerSource,
} from "@patchbay/core/automations";
import { Button } from "@patchbay/ui/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@patchbay/ui/components/ui/dropdown-menu";
import { Input } from "@patchbay/ui/components/ui/input";
import { cn } from "@patchbay/ui/lib/utils";
import { GitHubMark } from "../../settings/components/github-mark";
import { LinearMark } from "../../settings/components/linear-mark";
import { SlackMark } from "../../settings/components/slack-mark";
import { useT } from "../../i18n";

export function AutomationTriggerSourceGlyph({
  source,
}: {
  source: AutomationTriggerSource["id"];
}): ReactNode {
  const className = "size-3.5 shrink-0 text-muted-foreground";
  switch (source) {
    case "scheduled":
      return <Clock className={className} />;
    case "github":
      return <GitHubMark className={className} />;
    case "slack":
      return <SlackMark className={className} />;
    case "linear":
      return <LinearMark className={className} />;
    default:
      return <Webhook className={className} />;
  }
}

export function TriggerAddMenu({
  canWrite,
  onPickSchedule,
  onPickPreset,
  variant = "toolbar",
}: {
  canWrite: boolean;
  onPickSchedule: () => void;
  onPickPreset: (preset: AutomationTriggerPreset) => void;
  /** `inset` sits as the last row inside the Triggers card. */
  variant?: "toolbar" | "inset";
}) {
  const { t } = useT("automations");
  const [query, setQuery] = useState("");
  const filtered = useMemo(() => {
    const labels: Record<string, string> = {};
    for (const source of AUTOMATION_TRIGGER_SOURCES) {
      labels[source.labelKey] = t(
        ($) =>
          $.trigger_sources[source.labelKey as keyof typeof $.trigger_sources],
      );
    }
    for (const preset of AUTOMATION_TRIGGER_PRESETS) {
      labels[preset.labelKey] = t(
        ($) => $.presets[preset.labelKey as keyof typeof $.presets],
      );
    }
    return searchTriggerCatalog(query, labels);
  }, [query, t]);
  const sourceLabel = (key: string) =>
    t(($) => $.trigger_sources[key as keyof typeof $.trigger_sources]) || key;
  const presetLabel = (key: string) =>
    t(($) => $.presets[key as keyof typeof $.presets]) || key;

  if (!canWrite) return null;

  return (
    <DropdownMenu
      onOpenChange={(open) => {
        if (!open) setQuery("");
      }}
    >
      <DropdownMenuTrigger
        render={
          <Button
            size="sm"
            variant="ghost"
            className={cn(
              "text-muted-foreground hover:text-foreground",
              variant === "inset" &&
                "h-8 w-full justify-start px-2 font-normal",
            )}
          >
            <Plus className="h-3.5 w-3.5" />
            {t(($) => $.settings.add_trigger)}
          </Button>
        }
      />
      <DropdownMenuContent
        align={variant === "inset" ? "start" : "end"}
        className="w-72 p-1.5"
      >
        <div className="relative px-1 pb-1.5">
          <Search className="pointer-events-none absolute top-1/2 left-3.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={t(($) => $.settings.search_triggers)}
            aria-label={t(($) => $.settings.search_triggers)}
            onKeyDown={(event) => {
              // Keep menu typeahead from stealing typed characters from search.
              if (event.key.length === 1 || event.key === "Backspace")
                event.stopPropagation();
            }}
            className="h-8 pl-7"
            autoFocus
          />
        </div>
        {filtered.sources.length === 0 && (
          <p
            role="status"
            className="px-2 py-3 text-caption text-muted-foreground"
          >
            {t(($) => $.settings.no_matching_triggers)}
          </p>
        )}
        {filtered.sources.map((source) => {
          const label = sourceLabel(source.labelKey);
          if (!source.nested) {
            return (
              <DropdownMenuItem
                key={source.id}
                onClick={() => {
                  if (source.id === "scheduled") {
                    onPickSchedule();
                    return;
                  }
                  const preset = presetsForSource(source.id)[0];
                  if (preset) onPickPreset(preset);
                }}
              >
                <AutomationTriggerSourceGlyph source={source.id} />
                <span>{label}</span>
              </DropdownMenuItem>
            );
          }
          const presets = filtered.presets.filter(
            (preset) => preset.source === source.id,
          );
          if (presets.length === 0) return null;
          const core = presets.filter(
            (preset) => preset.group !== "github_only",
          );
          const extra = presets.filter(
            (preset) => preset.group === "github_only",
          );
          return (
            <DropdownMenuSub key={source.id}>
              <DropdownMenuSubTrigger>
                <AutomationTriggerSourceGlyph source={source.id} />
                <span className="flex-1">{label}</span>
              </DropdownMenuSubTrigger>
              <DropdownMenuSubContent className="max-h-72 min-w-56 overflow-y-auto">
                {core.map((preset) => (
                  <DropdownMenuItem
                    key={preset.id}
                    onClick={() => onPickPreset(preset)}
                  >
                    {presetLabel(preset.labelKey)}
                  </DropdownMenuItem>
                ))}
                {extra.length > 0 && (
                  <>
                    <DropdownMenuSeparator />
                    <DropdownMenuGroup>
                      <DropdownMenuLabel>
                        {t(($) => $.presets_group.github_only)}
                      </DropdownMenuLabel>
                      {extra.map((preset) => (
                        <DropdownMenuItem
                          key={preset.id}
                          onClick={() => onPickPreset(preset)}
                        >
                          {presetLabel(preset.labelKey)}
                        </DropdownMenuItem>
                      ))}
                    </DropdownMenuGroup>
                  </>
                )}
              </DropdownMenuSubContent>
            </DropdownMenuSub>
          );
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
