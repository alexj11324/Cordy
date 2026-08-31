"use client";

import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Bot } from "lucide-react";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { isAgentRuntimeBound } from "@patchbay/core/agents";
import { agentListOptions, teamListOptions } from "@patchbay/core/workspace/queries";
import type { AutomationExecutorType } from "@patchbay/core/types";
import { ActorAvatar } from "../../../common/actor-avatar";
import {
  PropertyPicker,
  PickerItem,
  PickerSection,
  PickerEmpty,
} from "../../../issues/components/pickers/property-picker";
import { useT } from "../../../i18n";
import { matchesPinyin } from "../../../editor/extensions/pinyin-match";

export interface ExecutorSelection {
  type: AutomationExecutorType;
  id: string;
}

export function AgentPicker({
  executor,
  onChange,
  trigger: customTrigger,
  triggerRender,
  align = "start",
}: {
  executor: ExecutorSelection | null;
  onChange: (next: ExecutorSelection) => void;
  trigger?: React.ReactNode;
  triggerRender?: React.ReactElement;
  align?: "start" | "center" | "end";
}) {
  const { t } = useT("automations");
  const wsId = useWorkspaceId();
  const [open, setOpen] = useState(false);
  const [filter, setFilter] = useState("");
  const { data: agents = [] } = useQuery(agentListOptions(wsId));
  const { data: teams = [] } = useQuery(teamListOptions(wsId));

  const activeAgents = useMemo(() => agents.filter((a) => !a.archived_at), [agents]);
  const activeTeams = useMemo(() => teams.filter((s) => !s.archived_at), [teams]);
  const agentsById = useMemo(
    () => new Map(activeAgents.map((agent) => [agent.id, agent])),
    [activeAgents],
  );

  const selectedAgent =
    executor?.type === "agent" ? activeAgents.find((a) => a.id === executor.id) : undefined;
  const selectedTeam =
    executor?.type === "team" ? activeTeams.find((s) => s.id === executor.id) : undefined;
  const selectedName = selectedAgent?.name ?? selectedTeam?.name;

  const query = filter.trim().toLowerCase();
  const matches = (name: string) =>
    !query || name.toLowerCase().includes(query) || matchesPinyin(name, query);
  const filteredAgents = activeAgents.filter((a) => matches(a.name));
  const filteredTeams = activeTeams.filter((s) => matches(s.name));

  const isSelected = (type: AutomationExecutorType, id: string) =>
    executor?.type === type && executor?.id === id;

  const handlePick = (type: AutomationExecutorType, id: string) => {
    onChange({ type, id });
    setOpen(false);
  };

  return (
    <PropertyPicker
      open={open}
      onOpenChange={setOpen}
      width="w-56"
      align={align}
      searchable
      searchPlaceholder={t(($) => $.agent_picker.filter_placeholder)}
      onSearchChange={setFilter}
      triggerRender={triggerRender}
      trigger={
        customTrigger ?? (
          <>
            {executor && (selectedAgent || selectedTeam) ? (
              <>
                <ActorAvatar
                  actorType={executor.type}
                  actorId={executor.id}
                  size="sm"
                  showStatusDot={executor.type === "agent"}
                />
                <span className="truncate">{selectedName}</span>
              </>
            ) : (
              <>
                <Bot className="size-3" />
                <span>{t(($) => $.agent_picker.select_executor)}</span>
              </>
            )}
          </>
        )
      }
    >
      {filteredAgents.length === 0 && filteredTeams.length === 0 ? (
        <PickerEmpty />
      ) : (
        <>
          {filteredAgents.length > 0 && (
            <PickerSection label={t(($) => $.agent_picker.agents_group)}>
              {filteredAgents.map((a) => {
                const runtimeBound = isAgentRuntimeBound(a);
                return (
                  <PickerItem
                    key={a.id}
                    selected={isSelected("agent", a.id)}
                    disabled={!runtimeBound}
                    tooltip={
                      runtimeBound
                        ? undefined
                        : t(($) => $.agent_picker.agent_runtime_required)
                    }
                    onClick={() => handlePick("agent", a.id)}
                  >
                    <ActorAvatar actorType="agent" actorId={a.id} size="sm" showStatusDot />
                    <span className="truncate">{a.name}</span>
                  </PickerItem>
                );
              })}
            </PickerSection>
          )}
          {filteredTeams.length > 0 && (
            <PickerSection label={t(($) => $.agent_picker.teams_group)}>
              {filteredTeams.map((s) => {
                const leader = agentsById.get(s.leader_id);
                const runtimeBound = !!leader && isAgentRuntimeBound(leader);
                return (
                  <PickerItem
                    key={s.id}
                    selected={isSelected("team", s.id)}
                    disabled={!runtimeBound}
                    tooltip={
                      runtimeBound
                        ? undefined
                        : t(($) => $.agent_picker.team_runtime_required)
                    }
                    onClick={() => handlePick("team", s.id)}
                  >
                    <ActorAvatar actorType="team" actorId={s.id} size="sm" />
                    <span className="truncate">{s.name}</span>
                  </PickerItem>
                );
              })}
            </PickerSection>
          )}
        </>
      )}
    </PropertyPicker>
  );
}
