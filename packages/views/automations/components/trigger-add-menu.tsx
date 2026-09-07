"use client";

import { useMemo, useState } from "react";
import { Clock, GitBranch, MessageSquare, Plus, Webhook } from "lucide-react";
import {
  presetsForSource,
  searchTriggerCatalog,
  type AutomationTriggerPreset,
  type AutomationTriggerSource,
} from "@orvilo/core/automations";
import { Button } from "@orvilo/ui/components/ui/button";
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
} from "@orvilo/ui/components/ui/dropdown-menu";
import { Input } from "@orvilo/ui/components/ui/input";
import { useT } from "../../i18n";

function sourceIcon(source: AutomationTriggerSource["id"]) {
  switch (source) {
    case "scheduled":
      return Clock;
    case "github":
      return GitBranch;
    case "slack":
      return MessageSquare;
    case "linear":
      return GitBranch;
    default:
      return Webhook;
  }
}

export function TriggerAddMenu({
  canWrite,
  onPickSchedule,
  onPickPreset,
}: {
  canWrite: boolean;
  onPickSchedule: () => void;
  onPickPreset: (preset: AutomationTriggerPreset) => void;
}) {
  const { t } = useT("automations");
  const [query, setQuery] = useState("");
  const filtered = useMemo(() => searchTriggerCatalog(query), [query]);
  const sourceLabel = (key: string) =>
    t(($) => $.trigger_sources[key as keyof typeof $.trigger_sources]) || key;
  const presetLabel = (key: string) =>
    t(($) => $.presets[key as keyof typeof $.presets]) || key;

  if (!canWrite) return null;

  return (
    <DropdownMenu onOpenChange={(open) => { if (!open) setQuery(""); }}>
      <DropdownMenuTrigger
        render={
          <Button size="sm" variant="outline">
            <Plus className="h-3.5 w-3.5 mr-1" />
            {t(($) => $.settings.add_trigger)}
          </Button>
        }
      />
      <DropdownMenuContent align="end" className="w-64">
        <div className="px-2 pb-2">
          <Input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={t(($) => $.settings.search_triggers)}
            className="h-8"
            autoFocus
          />
        </div>
        {filtered.sources.map((source) => {
          const Icon = sourceIcon(source.id);
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
                <Icon className="size-3.5 text-muted-foreground" />
                <span>{label}</span>
              </DropdownMenuItem>
            );
          }
          const presets = filtered.presets.filter((preset) => preset.source === source.id);
          if (presets.length === 0) return null;
          const core = presets.filter((preset) => preset.group !== "github_only");
          const extra = presets.filter((preset) => preset.group === "github_only");
          return (
            <DropdownMenuSub key={source.id}>
              <DropdownMenuSubTrigger>
                <Icon className="size-3.5 text-muted-foreground" />
                <span className="flex-1">{label}</span>
              </DropdownMenuSubTrigger>
              <DropdownMenuSubContent className="max-h-72 min-w-56 overflow-y-auto">
                {core.map((preset) => (
                  <DropdownMenuItem key={preset.id} onClick={() => onPickPreset(preset)}>
                    {presetLabel(preset.labelKey)}
                  </DropdownMenuItem>
                ))}
                {extra.length > 0 && (
                  <>
                    <DropdownMenuSeparator />
                    <DropdownMenuGroup>
                      <DropdownMenuLabel>{t(($) => $.presets_group.github_only)}</DropdownMenuLabel>
                      {extra.map((preset) => (
                        <DropdownMenuItem key={preset.id} onClick={() => onPickPreset(preset)}>
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
