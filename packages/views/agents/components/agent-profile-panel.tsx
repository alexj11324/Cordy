"use client";

import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  Activity,
  Bot,
  ChevronRight,
  Clock3,
  CopyPlus,
  Cpu,
  Fingerprint,
  Gauge,
  Globe2,
  Lock,
  MessageSquare,
  Server,
  Settings2,
  UserRound,
  Wrench,
  X,
  type LucideIcon,
} from "lucide-react";
import {
  effectiveAccessScope,
  providerSupportsMcpConfig,
} from "@patchbay/core/agents";
import { runtimeDisplayLabel } from "@patchbay/core/runtimes";
import { useWorkspacePaths } from "@patchbay/core/paths";
import { Button } from "@patchbay/ui/components/ui/button";
import {
  Tabs,
  TabsList,
  TabsTrigger,
  TabsContent,
} from "@patchbay/ui/components/ui/tabs";
import { cn } from "@patchbay/ui/lib/utils";
import { AppLink } from "../../navigation";
import { ActorAvatar } from "../../common/actor-avatar";
import { AgentPresenceIndicator } from "./agent-presence-indicator";
import type { AgentListRow } from "./agents-page";
import { useLocale, useT } from "../../i18n";
import { availabilityConfig } from "../presence";

type ProfileTab = "info" | "runtime" | "capabilities" | "work";

const PROFILE_TABS: Array<{ id: ProfileTab; labelKey: ProfileTab }> = [
  { id: "info", labelKey: "info" },
  { id: "runtime", labelKey: "runtime" },
  { id: "capabilities", labelKey: "capabilities" },
  { id: "work", labelKey: "work" },
];

/**
 * Buzz's profile panel is a focused reading surface: a centered identity
 * hero, a segmented tab bar, and grouped rows that lead to deeper views. The
 * panel deliberately does not embed Cordy's editors; those remain on the
 * existing detail route reached through Edit or a row.
 */
export function AgentProfilePanel({
  row,
  onClose,
}: {
  row: AgentListRow;
  onClose: () => void;
}) {
  const { t } = useT("agents");
  const locale = useLocale();
  const paths = useWorkspacePaths();
  const [activeTab, setActiveTab] = useState<ProfileTab>("info");
  const [isMobile, setIsMobile] = useState(false);
  const panelRef = useRef<HTMLElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  const { agent, runtime, presence, owner } = row;

  useEffect(() => {
    setActiveTab("info");
  }, [agent.id]);

  useEffect(() => {
    const media = window.matchMedia?.("(max-width: 1023px)");
    if (!media) return;
    const update = () => setIsMobile(media.matches);
    update();
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);

  useEffect(() => {
    const panel = panelRef.current;
    const previous =
      isMobile && document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    const focusables = () =>
      Array.from(
        panel?.querySelectorAll<HTMLElement>(
          'a[href],button:not([disabled]),[tabindex]:not([tabindex="-1"])',
        ) ?? [],
      );
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onCloseRef.current();
        return;
      }
      if (!isMobile || !panel || event.key !== "Tab") return;
      const items = focusables();
      if (items.length === 0) {
        event.preventDefault();
        panel.focus();
        return;
      }
      const index = items.indexOf(document.activeElement as HTMLElement);
      if (event.shiftKey && (index <= 0 || index === -1)) {
        event.preventDefault();
        items.at(-1)?.focus();
      } else if (
        !event.shiftKey &&
        (index === -1 || index === items.length - 1)
      ) {
        event.preventDefault();
        items[0]?.focus();
      }
    };
    document.addEventListener("keydown", handleKeyDown);
    if (isMobile && panel) (focusables()[0] ?? panel).focus();
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
      previous?.focus();
    };
  }, [agent.id, isMobile]);

  const detailHref = paths.agentDetail(agent.id);
  const detailWithView = (view: string) => `${detailHref}?view=${view}`;
  const duplicateHref = `${paths.newAgentManual()}?duplicate=${encodeURIComponent(agent.id)}`;
  const description =
    agent.description.trim() ||
    t(($) => $.inspector.no_description_placeholder);
  const access = effectiveAccessScope(
    agent.permission_mode,
    agent.invocation_targets,
  );
  const accessLabel =
    access === "workspace"
      ? t(($) => $.access.scope_labels.workspace)
      : access === "specific-people"
        ? t(($) => $.access.scope_labels.specific_people)
        : t(($) => $.access.scope_labels.owner_only);
  const hasMcp = runtime ? providerSupportsMcpConfig(runtime.provider) : true;
  const mcpValue = agent.mcp_config_redacted
    ? t(($) => $.profile_panel.configured)
    : agent.mcp_config !== null && agent.mcp_config !== undefined
      ? t(($) => $.profile_panel.configured)
      : t(($) => $.profile_panel.not_configured);
  const lastActive =
    row.lastActiveDays === null
      ? t(($) => $.last_active.none)
      : row.lastActiveDays === 0
        ? t(($) => $.last_active.today)
        : t(($) => $.last_active.days_ago, { count: row.lastActiveDays });

  return (
    <>
      <div
        aria-hidden="true"
        className="fixed inset-0 z-40 bg-black/10 lg:hidden"
        onClick={onClose}
      />
      <aside
        aria-labelledby="agent-profile-panel-title"
        aria-modal={isMobile ? true : undefined}
        className="fixed inset-y-0 right-0 z-50 flex w-full max-w-[420px] min-w-0 flex-col border-l border-border/70 bg-background shadow-xl lg:relative lg:inset-auto lg:z-auto lg:w-[400px] lg:max-w-none lg:shadow-none"
        data-testid="agent-profile-panel"
        ref={panelRef}
        role="dialog"
        tabIndex={-1}
      >
        <header className="flex min-h-12 shrink-0 items-center justify-between gap-3 border-b px-4">
          <h2
            className="min-w-0 truncate text-title-sm font-semibold tracking-tight"
            id="agent-profile-panel-title"
          >
            {t(($) => $.profile_panel.title)}
          </h2>
          <div className="flex shrink-0 items-center gap-1">
            <Button
              className="text-body"
              nativeButton={false}
              render={
                <AppLink href={detailWithView("general")} onClick={onClose} />
              }
              size="sm"
              variant="ghost"
            >
              {t(($) => $.profile_panel.edit)}
            </Button>
            <Button
              aria-label={t(($) => $.profile_panel.close)}
              onClick={onClose}
              size="icon-sm"
              type="button"
              variant="ghost"
            >
              <X aria-hidden="true" />
            </Button>
          </div>
        </header>

        <div className="min-h-0 flex-1 overflow-y-auto px-4 pb-6">
          <div className="flex flex-col gap-6 pt-5">
            <ProfileHero
              agent={agent}
              description={description}
              presence={presence}
            />

            <Tabs
              className="w-full"
              onValueChange={(value) => setActiveTab(value as ProfileTab)}
              value={activeTab}
            >
              <TabsList
                aria-label={t(($) => $.profile_panel.title)}
                className="grid h-9 w-full grid-cols-4 overflow-hidden rounded-lg bg-muted p-0.5"
              >
                {PROFILE_TABS.map((tab) => (
                  <TabsTrigger
                    className="h-full min-w-0 rounded-md px-2 text-caption"
                    key={tab.id}
                    value={tab.id}
                  >
                    <span className="truncate">
                      {t(($) => $.profile_panel[`tab_${tab.labelKey}`])}
                    </span>
                  </TabsTrigger>
                ))}
              </TabsList>

              <TabsContent className="mt-5 outline-none" value="info">
                <div className="space-y-5">
                  <ProfileSection title={t(($) => $.profile_panel.tab_info)}>
                    <ProfileRow
                      href={detailWithView("instructions")}
                      icon={MessageSquare}
                      label={t(($) => $.profile_panel.agent_instructions)}
                      onClick={onClose}
                      value={
                        agent.instructions.trim() ||
                        t(($) => $.profile_panel.no_instruction)
                      }
                      valueClassName={
                        agent.instructions.trim() ? "line-clamp-2" : undefined
                      }
                    />
                    <ProfileRow
                      icon={Fingerprint}
                      label={t(($) => $.profile_panel.agent_id)}
                      value={truncateIdentifier(agent.id)}
                    />
                    <ProfileRow
                      icon={UserRound}
                      label={t(($) => $.inspector.prop_owner)}
                      value={
                        owner ? (
                          <span className="flex min-w-0 items-center gap-1.5">
                            <ActorAvatar
                              actorId={owner.user_id}
                              actorType="member"
                              profileLink={false}
                              size="sm"
                            />
                            <span className="truncate">{owner.name}</span>
                          </span>
                        ) : (
                          "—"
                        )
                      }
                    />
                    <ProfileRow
                      icon={access === "owner-only" ? Lock : Globe2}
                      label={t(($) => $.tabs.access)}
                      value={accessLabel}
                    />
                    <ProfileRow
                      icon={Cpu}
                      label={t(($) => $.profile_panel.agent_type)}
                      value={t(($) => $.profile_panel.agent_type_value)}
                    />
                  </ProfileSection>

                  <ProfileSection>
                    {!agent.archived_at ? (
                      <ProfileRow
                        href={duplicateHref}
                        icon={CopyPlus}
                        label={t(($) => $.row_actions.duplicate)}
                        onClick={onClose}
                      />
                    ) : null}
                    <ProfileRow
                      href={detailWithView("general")}
                      icon={Settings2}
                      label={t(($) => $.profile_panel.open_details)}
                      onClick={onClose}
                    />
                  </ProfileSection>
                </div>
              </TabsContent>

              <TabsContent className="mt-5 outline-none" value="runtime">
                <div className="space-y-5">
                  <ProfileSection title={t(($) => $.profile_panel.tab_runtime)}>
                    <ProfileRow
                      icon={Activity}
                      label={t(($) => $.columns.status)}
                      value={
                        agent.archived_at ? (
                          t(($) => $.row.archived)
                        ) : (
                          <AgentPresenceIndicator detail={presence} />
                        )
                      }
                    />
                    <ProfileRow
                      href={detailWithView("general")}
                      icon={Server}
                      label={t(($) => $.inspector.prop_runtime)}
                      onClick={onClose}
                      value={
                        runtime
                          ? runtimeDisplayLabel(runtime)
                          : t(($) => $.pickers.runtime_none)
                      }
                    />
                    <ProfileRow
                      href={detailWithView("general")}
                      icon={Bot}
                      label={t(($) => $.inspector.prop_model)}
                      onClick={onClose}
                      value={agent.model || t(($) => $.pickers.model_default)}
                    />
                    <ProfileRow
                      icon={Gauge}
                      label={t(($) => $.inspector.prop_concurrency)}
                      value={agent.max_concurrent_tasks.toLocaleString(locale)}
                    />
                  </ProfileSection>

                  <ProfileSection title={t(($) => $.tabs.work)}>
                    <ProfileRow
                      icon={Clock3}
                      label={t(($) => $.columns.last_active)}
                      value={lastActive}
                    />
                    <ProfileRow
                      icon={Activity}
                      label={t(($) => $.columns.runs)}
                      value={row.runCount.toLocaleString(locale)}
                    />
                  </ProfileSection>
                </div>
              </TabsContent>

              <TabsContent className="mt-5 outline-none" value="capabilities">
                <div className="space-y-5">
                  <ProfileSection
                    title={t(($) => $.profile_panel.tab_capabilities)}
                  >
                    <ProfileRow
                      href={detailWithView("skills")}
                      icon={Wrench}
                      label={t(($) => $.tabs.skills)}
                      onClick={onClose}
                      value={String(agent.skills.length)}
                    />
                    {hasMcp ? (
                      <ProfileRow
                        href={detailWithView("mcp_config")}
                        icon={Settings2}
                        label={t(($) => $.tabs.mcp_config)}
                        onClick={onClose}
                        value={mcpValue}
                      />
                    ) : null}
                  </ProfileSection>
                </div>
              </TabsContent>

              <TabsContent className="mt-5 outline-none" value="work">
                <div className="space-y-5">
                  <ProfileSection title={t(($) => $.profile_panel.tab_work)}>
                    <ProfileRow
                      href={detailWithView("work")}
                      icon={Activity}
                      label={t(($) => $.tabs.work)}
                      onClick={onClose}
                      value={
                        row.runCount > 0
                          ? t(($) => $.tab_body.activity.runs, {
                              count: row.runCount,
                            })
                          : t(($) => $.tab_body.activity.empty_recent)
                      }
                    />
                    <ProfileRow
                      icon={Clock3}
                      label={t(($) => $.columns.last_active)}
                      value={lastActive}
                    />
                  </ProfileSection>
                </div>
              </TabsContent>
            </Tabs>
          </div>
        </div>
      </aside>
    </>
  );
}

function ProfileHero({
  agent,
  description,
  presence,
}: {
  agent: AgentListRow["agent"];
  description: string;
  presence: AgentListRow["presence"];
}) {
  const { t } = useT("agents");
  const availability = agent.archived_at
    ? "archived"
    : (presence?.availability ?? null);
  const visual =
    availability === null ? null : availabilityConfig[availability];

  return (
    <div className="flex flex-col items-center gap-3 text-center">
      <div className="relative flex h-24 w-24 items-center justify-center">
        <ActorAvatar
          actorId={agent.id}
          actorType="agent"
          className="scale-[1.7] bg-background ring-2 ring-background shadow-sm"
          profileLink={false}
          size="2xl"
        />
        {availability !== null && visual ? (
          <span
            aria-label={t(($) => $.availability[availability])}
            className={cn(
              "absolute right-1 bottom-1 size-4 rounded-full border-2 border-background",
              visual.dotClass,
            )}
            role="img"
            title={t(($) => $.availability[availability])}
          />
        ) : null}
      </div>
      <div className="flex max-w-full flex-col items-center gap-1">
        <h3 className="max-w-full truncate text-title-lg font-semibold tracking-tight">
          {agent.name}
        </h3>
        <p className="line-clamp-2 max-w-[320px] text-caption text-muted-foreground">
          {description}
        </p>
      </div>
    </div>
  );
}

function ProfileSection({
  children,
  title,
}: {
  children: ReactNode;
  title?: string;
}) {
  return (
    <section className="space-y-2">
      {title ? (
        <h3 className="px-4 text-caption font-semibold text-muted-foreground">
          {title}
        </h3>
      ) : null}
      <div className="divide-y divide-border/55 overflow-hidden rounded-xl border border-border/70 bg-background/70">
        {children}
      </div>
    </section>
  );
}

function ProfileRow({
  href,
  icon: Icon,
  label,
  onClick,
  value,
  valueClassName,
}: {
  href?: string;
  icon: LucideIcon;
  label: string;
  onClick?: () => void;
  value?: ReactNode;
  valueClassName?: string;
}) {
  const content = (
    <>
      <Icon
        aria-hidden="true"
        className="size-4 shrink-0 text-muted-foreground"
      />
      <span className="min-w-0 flex-1 text-left">
        <span className="block text-body font-medium text-foreground">
          {label}
        </span>
        {value !== undefined ? (
          <span
            className={cn(
              "mt-0.5 block truncate text-body text-muted-foreground",
              valueClassName,
            )}
            title={typeof value === "string" ? value : undefined}
          >
            {value}
          </span>
        ) : null}
      </span>
      {href ? (
        <ChevronRight
          aria-hidden="true"
          className="size-4 shrink-0 text-muted-foreground"
        />
      ) : null}
    </>
  );

  if (href) {
    return (
      <AppLink
        className="group flex min-h-16 w-full items-center gap-3 px-4 py-3 text-left transition-colors hover:bg-muted/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
        href={href}
        onClick={onClick}
      >
        {content}
      </AppLink>
    );
  }

  return (
    <div className="flex min-h-16 w-full items-center gap-3 px-4 py-3">
      {content}
    </div>
  );
}

function truncateIdentifier(value: string): string {
  if (value.length <= 16) return value;
  return `${value.slice(0, 8)}…${value.slice(-4)}`;
}
