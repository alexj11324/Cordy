"use client";

// Provider rail and favorites presentation adapted from T3 Code (MIT).
// See third-party/t3code.md and third-party/t3code-LICENSE.
import { useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Check,
  ChevronRight,
  Loader2,
  RefreshCw,
  Search,
  Star,
} from "lucide-react";
import {
  runtimeDisplayLabel,
  runtimeModelsOptions,
} from "@orvilo/core/runtimes";
import {
  modelFavoriteKey,
  useModelFavoritesStore,
  type ModelFavorite,
} from "@orvilo/core/agents/stores";
import type { RuntimeDevice, RuntimeModel } from "@orvilo/core/types";
import { Input } from "@orvilo/ui/components/ui/input";
import { cn } from "@orvilo/ui/lib/utils";
import { ProviderLogo } from "../../runtimes/components/provider-logo";
import { useT } from "../../i18n";
import {
  groupModelSelectorOptions,
  nativeEffortLabel,
  explicitThinkingLevel,
  explicitServiceTier,
} from "./model-selector-options";
import { ModelSpeedColumn } from "./model-speed-column";
import { buildModelChangeUpdate } from "./inspector/model-change-cleanup";
import { findModelCapabilityEntry } from "./inspector/model-capability";

export type ModelSelectorRuntime = Pick<
  RuntimeDevice,
  "id" | "provider" | "name" | "custom_name" | "status"
> &
  Partial<Pick<RuntimeDevice, "workspace_id">> & {
    /** False for the agent's current runtime when the viewer cannot use it. */
    selectable?: boolean;
  };
export type ModelSelection = ModelFavorite & {
  catalog: RuntimeModel[] | null;
  serviceTier: string;
};

export function ModelSelectorContent({
  runtimes,
  runtimeId,
  model,
  thinkingLevel,
  serviceTier = "",
  onSelect,
  allowEffort = true,
  allowSpeed = allowEffort,
  className,
  autoFocus = true,
  preferFavorites = true,
}: {
  runtimes: ModelSelectorRuntime[];
  runtimeId: string;
  model: string;
  thinkingLevel: string;
  serviceTier?: string;
  onSelect: (selection: ModelSelection) => Promise<void> | void;
  allowEffort?: boolean;
  allowSpeed?: boolean;
  className?: string;
  autoFocus?: boolean;
  preferFavorites?: boolean;
}) {
  const { t } = useT("agents");
  const favorites = useModelFavoritesStore((state) => state.favorites);
  const toggle = useModelFavoritesStore((state) => state.toggle);
  const availableFavorites = favorites.filter(
    (favorite) =>
      runtimes.some((runtime) => runtime.id === favorite.runtimeId) &&
      runtimes.find((runtime) => runtime.id === favorite.runtimeId)
        ?.selectable !== false &&
      (allowEffort || favorite.thinkingLevel === thinkingLevel),
  );
  const initialRuntimeId = runtimes.some((item) => item.id === runtimeId)
    ? runtimeId
    : (runtimes[0]?.id ?? "");
  const [section, setSection] = useState(() =>
    preferFavorites && availableFavorites.length
      ? "favorites"
      : initialRuntimeId,
  );
  const [browsingRuntimeId, setBrowsingRuntimeId] = useState(initialRuntimeId);
  const [browsingModel, setBrowsingModel] = useState(
    initialRuntimeId === runtimeId ? model : "",
  );
  const [search, setSearch] = useState("");
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  const [refreshAnimating, setRefreshAnimating] = useState(false);
  const busy = useRef(false);
  const queryClient = useQueryClient();
  const runtime = runtimes.find((item) => item.id === browsingRuntimeId);
  const modelsQuery = useQuery(
    runtimeModelsOptions(
      runtime?.status === "online" && runtime.selectable !== false
        ? runtime.id
        : null,
      runtime?.workspace_id,
    ),
  );
  const models = modelsQuery.data?.models ?? [];
  const supported = modelsQuery.data?.supported !== false;
  const entry = findModelCapabilityEntry(
    models,
    browsingModel,
    runtime?.provider ?? "",
  );
  const levels = entry?.thinking?.supported_levels ?? [];
  const needle = search.trim().toLowerCase();
  const matches = (item: RuntimeModel) =>
    `${item.label} ${item.id} ${item.provider ?? ""}`
      .toLowerCase()
      .includes(needle);
  const modelOptions = groupModelSelectorOptions(
    models,
    runtime?.provider ?? "",
  );
  const filtered = modelOptions.filter(
    (option) => matches(option) || option.variants?.some(matches),
  );
  const browsingOption = modelOptions.find(
    (option) =>
      option.id === browsingModel ||
      option.variants?.some((variant) => variant.id === browsingModel),
  );
  const canCreate =
    needle.length > 0 &&
    !models.some(
      (item) => item.id === search.trim() || item.label === search.trim(),
    );
  const catalog = modelsQuery.isSuccess ? models : null;
  const isFavorite = (choice: ModelFavorite) =>
    favorites.some(
      (item) => modelFavoriteKey(item) === modelFavoriteKey(choice),
    );

  function compatibleSpeed(
    choice: ModelFavorite,
    selectedCatalog: RuntimeModel[] | null,
  ) {
    if (choice.runtimeId !== runtimeId) return "";
    return (
      buildModelChangeUpdate({
        provider:
          runtimes.find((item) => item.id === choice.runtimeId)?.provider ?? "",
        model: choice.model,
        thinkingLevel: choice.thinkingLevel,
        serviceTier,
        catalog: selectedCatalog,
      }).service_tier ?? serviceTier
    );
  }

  function favoriteSpeed(
    choice: ModelFavorite,
    selectedEntry: RuntimeModel | undefined,
    selectedCatalog: RuntimeModel[] | null,
  ) {
    const stored = choice.serviceTier ?? "";
    if (!stored) return compatibleSpeed(choice, selectedCatalog);
    if (stored === "default") {
      if (selectedEntry?.supports_explicit_standard_service_tier === true) {
        return "default";
      }
      throw new Error(t(($) => $.model_selector.favorite_unavailable));
    }
    if (
      (selectedEntry?.service_tiers ?? []).some((tier) => tier.id === stored)
    ) {
      return stored;
    }
    throw new Error(t(($) => $.model_selector.favorite_unavailable));
  }

  function speedLabelFor(choice: ModelFavorite, catalogEntry?: RuntimeModel) {
    if (!allowSpeed) return "";
    const stored = choice.serviceTier ?? "";
    if (!stored) return "";
    if (stored === "default") {
      return t(($) => $.pickers.service_tier_standard);
    }
    return (
      catalogEntry?.service_tiers?.find((tier) => tier.id === stored)?.name ??
      choice.serviceTierLabel ??
      stored
    );
  }

  function combinationLabel(
    modelLabel: string,
    effortLabel: string,
    choice: ModelFavorite,
    catalogEntry?: RuntimeModel,
  ) {
    const extras = [effortLabel, speedLabelFor(choice, catalogEntry)].filter(
      Boolean,
    );
    return extras.length ? `${modelLabel} (${extras.join(" · ")})` : modelLabel;
  }

  const browsingChoice: ModelFavorite = {
    runtimeId: browsingRuntimeId,
    model: browsingModel,
    thinkingLevel:
      browsingRuntimeId === runtimeId && browsingModel === model
        ? thinkingLevel
        : "",
  };
  const browsingServiceTier =
    browsingRuntimeId === runtimeId && browsingModel === model
      ? serviceTier
      : compatibleSpeed(browsingChoice, catalog);
  const currentChoice = (effort: string): ModelFavorite => ({
    runtimeId: browsingRuntimeId,
    model: browsingModel,
    thinkingLevel: effort,
    serviceTier: allowSpeed ? (browsingServiceTier ?? "") : "",
  });

  async function select(
    choice: ModelFavorite & { serviceTier?: string },
    fromFavorite = false,
  ) {
    if (busy.current) return;
    busy.current = true;
    setSaving(true);
    setError("");
    try {
      const targetRuntime = runtimes.find(
        (item) => item.id === choice.runtimeId,
      );
      if (targetRuntime?.selectable === false) return;
      let selectedCatalog = catalog;
      let selectedEntry = findModelCapabilityEntry(
        catalog ?? [],
        choice.model,
        runtimes.find((item) => item.id === choice.runtimeId)?.provider ?? "",
      );
      if (fromFavorite) {
        const target = runtimes.find((item) => item.id === choice.runtimeId);
        if (target?.status !== "online")
          throw new Error(t(($) => $.model_selector.favorite_unavailable));
        const result = await queryClient.fetchQuery(
          runtimeModelsOptions(target.id, target.workspace_id),
        );
        selectedCatalog = result.models;
        selectedEntry = findModelCapabilityEntry(
          result.models,
          choice.model,
          target.provider,
        );
        if (
          !result.supported ||
          !selectedEntry ||
          (choice.thinkingLevel &&
            !selectedEntry.thinking?.supported_levels.some(
              (level) => level.value === choice.thinkingLevel,
            ))
        ) {
          throw new Error(t(($) => $.model_selector.favorite_unavailable));
        }
      }
      await onSelect({
        runtimeId: choice.runtimeId,
        model: choice.model,
        thinkingLevel: choice.thinkingLevel,
        serviceTier: allowSpeed
          ? fromFavorite
            ? favoriteSpeed(choice, selectedEntry, selectedCatalog)
            : (choice.serviceTier ?? compatibleSpeed(choice, selectedCatalog))
          : serviceTier,
        catalog: selectedCatalog,
      });
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      busy.current = false;
      setSaving(false);
    }
  }

  const star = (choice: ModelFavorite, label: string) => (
    <button
      type="button"
      aria-label={t(
        ($) =>
          isFavorite(choice)
            ? $.model_selector.unfavorite
            : $.model_selector.favorite,
        { value: label },
      )}
      aria-pressed={isFavorite(choice)}
      onClick={() => {
        const option = modelOptions.find(
          (item) =>
            item.id === choice.model ||
            item.variants?.some((variant) => variant.id === choice.model),
        );
        const variant = option?.variants?.find(
          (item) => item.id === choice.model,
        );
        toggle({
          runtimeId: choice.runtimeId,
          model: choice.model,
          thinkingLevel: choice.thinkingLevel,
          serviceTier: allowSpeed ? (choice.serviceTier ?? "") : "",
          modelLabel: option?.label ?? choice.modelLabel,
          thinkingLabel: variant
            ? nativeEffortLabel(variant)
            : (option?.thinking?.supported_levels.find(
                (level) => level.value === choice.thinkingLevel,
              )?.label ?? choice.thinkingLabel),
          serviceTierLabel: allowSpeed
            ? speedLabelFor(choice, entry)
            : undefined,
        });
      }}
      className="flex size-8 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
    >
      <Star
        className={cn(
          "size-3.5",
          isFavorite(choice) && "fill-current text-foreground",
        )}
        aria-hidden
      />
    </button>
  );
  const rowClass =
    "flex min-w-0 flex-1 items-center gap-2 rounded-md px-2 py-2.5 text-left text-caption hover:bg-accent/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";
  const showRuntime = (id: string) => {
    setSection(id);
    setBrowsingRuntimeId(id);
    setBrowsingModel(id === runtimeId ? model : "");
    setSearch("");
    setError("");
  };

  if (runtimes.length === 0) {
    return (
      <p
        role="status"
        className="px-3 py-8 text-center text-caption text-muted-foreground"
      >
        {t(($) => $.pickers.runtime_empty)}
      </p>
    );
  }

  return (
    <div
      data-model-selector
      className={cn(
        "flex h-80 max-h-[min(70vh,var(--available-height,28rem))] min-h-0",
        className,
      )}
      aria-busy={saving}
    >
      <nav
        aria-label={t(($) => $.model_selector.providers)}
        className="w-11 shrink-0 overflow-y-auto border-r border-border/60 bg-muted/30 [scrollbar-width:none]"
      >
        <div className="flex flex-col gap-1 p-1">
          <button
            type="button"
            aria-label={t(($) => $.model_selector.favorites)}
            aria-pressed={section === "favorites"}
            className={cn(
              "flex aspect-square w-full items-center justify-center rounded-md hover:bg-accent focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
              section === "favorites" &&
                "bg-primary/10 text-accent-foreground hover:bg-primary/15",
            )}
            onClick={() => {
              setSection("favorites");
              setSearch("");
              setError("");
            }}
          >
            <Star className="size-5 fill-current" aria-hidden />
          </button>
          <div className="border-b border-border/70" />
          {runtimes.map((item) => (
            <button
              key={item.id}
              type="button"
              title={runtimeDisplayLabel(item)}
              aria-label={runtimeDisplayLabel(item)}
              aria-pressed={section === item.id}
              disabled={item.selectable === false}
              onClick={() => showRuntime(item.id)}
              className={cn(
                "flex aspect-square w-full items-center justify-center rounded-md hover:bg-accent focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
                section === item.id &&
                  "bg-primary/10 text-accent-foreground hover:bg-primary/15",
              )}
            >
              <ProviderLogo provider={item.provider} className="size-5" />
            </button>
          ))}
        </div>
      </nav>
      <div className="flex min-w-0 flex-1 flex-col">
        <div className="flex h-11 shrink-0 items-center gap-2 border-b border-border/60 px-3">
          <Search
            className="size-4 shrink-0 text-muted-foreground"
            aria-hidden
          />
          <Input
            autoFocus={autoFocus}
            aria-label={t(($) => $.pickers.model_search_placeholder)}
            placeholder={t(($) => $.pickers.model_search_placeholder)}
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            disabled={
              section !== "favorites" && runtime?.selectable === false
            }
            className="h-8 border-0 bg-transparent px-0 shadow-none focus-visible:ring-0"
          />
          <button
            type="button"
            aria-label={t(($) => $.model_selector.refresh)}
            title={t(($) => $.model_selector.refresh)}
            disabled={
              runtime?.status !== "online" ||
              runtime.selectable === false ||
              modelsQuery.isFetching ||
              saving
            }
            aria-busy={modelsQuery.isFetching}
            onClick={() => {
              setRefreshAnimating(true);
              void modelsQuery.refetch();
            }}
            className="flex size-7 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
          >
            <RefreshCw
              aria-hidden
              className={cn(
                "size-3.5",
                (modelsQuery.isFetching || refreshAnimating) &&
                  "animate-spin motion-reduce:animate-none",
              )}
              onAnimationIteration={() => {
                if (!modelsQuery.isFetching) setRefreshAnimating(false);
              }}
            />
          </button>
        </div>
        {error && (
          <p
            role="alert"
            className="shrink-0 px-3 py-2 text-caption text-destructive"
          >
            {error}
          </p>
        )}
        {section === "favorites" ? (
          <div
            className="min-h-0 flex-1 overflow-y-auto p-1.5"
            aria-label={t(($) => $.model_selector.favorites)}
          >
            {availableFavorites
              .filter((favorite) => {
                const favoriteRuntime = runtimes.find(
                  (item) => item.id === favorite.runtimeId,
                );
                return `${favorite.model} ${favorite.modelLabel ?? ""} ${favorite.thinkingLevel} ${favorite.thinkingLabel ?? ""} ${favorite.serviceTier ?? ""} ${favorite.serviceTierLabel ?? ""} ${favoriteRuntime ? runtimeDisplayLabel(favoriteRuntime) : ""}`
                  .toLowerCase()
                  .includes(needle);
              })
              .map((favorite) => {
                const owner = runtimes.find(
                  (item) => item.id === favorite.runtimeId,
                )!;
                const favoriteEntry = findModelCapabilityEntry(
                  favorite.runtimeId === browsingRuntimeId
                    ? models
                    : (queryClient.getQueryData<{ models: RuntimeModel[] }>(
                        runtimeModelsOptions(favorite.runtimeId).queryKey,
                      )?.models ?? []),
                  favorite.model,
                  owner.provider,
                );
                const favoriteEffort =
                  favoriteEntry?.thinking?.supported_levels.find(
                    (level) => level.value === favorite.thinkingLevel,
                  )?.label ??
                  favorite.thinkingLabel ??
                  favorite.thinkingLevel;
                const selected =
                  modelFavoriteKey(favorite) ===
                  modelFavoriteKey({
                    runtimeId,
                    model,
                    thinkingLevel,
                    serviceTier: allowSpeed ? serviceTier : "",
                  });
                const label = combinationLabel(
                  favorite.modelLabel ?? favoriteEntry?.label ?? favorite.model,
                  favoriteEffort,
                  favorite,
                  favoriteEntry,
                );
                return (
                  <div
                    key={modelFavoriteKey(favorite)}
                    className={cn(
                      "flex items-center rounded-md hover:bg-accent/50",
                      selected &&
                        "bg-accent text-accent-foreground ring-1 ring-inset ring-border",
                    )}
                  >
                    <button
                      type="button"
                      disabled={saving}
                      aria-pressed={selected}
                      onClick={() => void select(favorite, true)}
                      className={rowClass}
                    >
                      <span className="min-w-0 flex-1">
                        <span className="block truncate font-medium">
                          {label}
                        </span>
                        <span className="mt-1 flex items-center gap-1.5 text-micro text-muted-foreground">
                          <ProviderLogo
                            provider={owner.provider}
                            className="size-3"
                          />
                          {runtimeDisplayLabel(owner)}
                        </span>
                      </span>
                      {selected && (
                        <Check aria-hidden className="size-4 shrink-0" />
                      )}
                    </button>
                    {star(favorite, label)}
                  </div>
                );
              })}
            {availableFavorites.length === 0 && (
              <p className="px-3 py-8 text-center text-caption text-muted-foreground">
                {t(($) => $.model_selector.favorites_empty)}
              </p>
            )}
          </div>
        ) : (
          <div className="grid min-h-0 min-w-0 flex-1 grid-cols-[minmax(12.5rem,1.4fr)_minmax(10rem,1fr)_minmax(10.5rem,1fr)] overflow-x-auto">
            <div
              className="min-w-0 flex-1 overflow-y-auto p-1.5"
              aria-label={t(($) => $.model_dropdown.label)}
            >
              <p className="px-2 py-1 text-micro text-muted-foreground">
                {runtime ? runtimeDisplayLabel(runtime) : null}
              </p>
              {modelsQuery.isLoading && (
                <p
                  role="status"
                  className="flex items-center gap-2 px-2 py-5 text-caption text-muted-foreground"
                >
                  <Loader2 className="size-4 animate-spin" />
                  {t(($) => $.pickers.model_discovering)}
                </p>
              )}
              {modelsQuery.isError && (
                <div className="px-2 py-3 text-caption text-muted-foreground">
                  <p>{t(($) => $.model_dropdown.discovery_failed)}</p>
                  <p className="mt-1 break-words">
                    {modelsQuery.error.message}
                  </p>
                  <p className="mt-2">
                    {t(($) => $.pickers.model_discovery_failed_hint)}
                  </p>
                </div>
              )}
              {!supported ? (
                <div className="px-2 py-5 text-caption text-muted-foreground">
                  <p>{t(($) => $.model_dropdown.managed_by_runtime_title)}</p>
                  <p className="mt-2">
                    {t(($) => $.model_dropdown.managed_by_runtime_hint)}
                  </p>
                  <button
                    type="button"
                    disabled={saving}
                    aria-pressed={browsingRuntimeId === runtimeId && !model}
                    className={cn(
                      rowClass,
                      "mt-3 w-full text-foreground",
                      browsingRuntimeId === runtimeId &&
                        !model &&
                        "bg-accent text-accent-foreground ring-1 ring-inset ring-border",
                    )}
                    onClick={() =>
                      void select({
                        runtimeId: browsingRuntimeId,
                        model: "",
                        thinkingLevel: "",
                        serviceTier: "",
                      })
                    }
                  >
                    <span className="min-w-0 flex-1">
                      {t(($) => $.pickers.bind_host_managed_runtime)}
                    </span>
                    {browsingRuntimeId === runtimeId && !model && (
                      <Check aria-hidden className="size-4 shrink-0" />
                    )}
                  </button>
                </div>
              ) : (
                <>
                  {filtered.map((item) => (
                    <button
                      key={item.id}
                      type="button"
                      disabled={saving}
                      aria-pressed={browsingOption?.id === item.id}
                      onClick={() => {
                        setBrowsingModel(item.id);
                        const nextEntry = findModelCapabilityEntry(
                          models,
                          item.id,
                          runtime?.provider ?? "",
                        );
                        const hasEffort =
                          Boolean(item.variants?.length) ||
                          (allowEffort &&
                            (nextEntry?.thinking?.supported_levels.length ??
                              0) > 0);
                        if (!hasEffort) {
                          void select({
                            runtimeId: browsingRuntimeId,
                            model: item.id,
                            thinkingLevel: explicitThinkingLevel(
                              nextEntry,
                              browsingRuntimeId === runtimeId &&
                                item.id === model
                                ? thinkingLevel
                                : "",
                            ),
                            serviceTier: allowSpeed
                              ? explicitServiceTier(
                                  nextEntry,
                                  browsingRuntimeId === runtimeId &&
                                    item.id === model
                                    ? serviceTier
                                    : "",
                                )
                              : serviceTier,
                          });
                        }
                      }}
                      className={cn(
                        rowClass,
                        "w-full",
                        browsingOption?.id === item.id &&
                          "bg-accent text-accent-foreground ring-1 ring-inset ring-border",
                      )}
                    >
                      <span className="min-w-0 flex-1">
                        <span className="block truncate font-medium">
                          {item.label}
                        </span>
                        <span className="mt-0.5 block truncate text-micro text-muted-foreground">
                          {item.variants
                            ? item.id.replace(/-(high|medium|low)$/, "")
                            : item.id}
                        </span>
                      </span>
                      {browsingOption?.id === item.id ? (
                        <Check aria-hidden className="size-4 shrink-0" />
                      ) : (
                        <ChevronRight
                          aria-hidden
                          className="size-3.5 shrink-0 text-muted-foreground"
                        />
                      )}
                    </button>
                  ))}
                  {canCreate && (
                    <button
                      type="button"
                      disabled={saving}
                      className={cn(rowClass, "w-full text-primary")}
                      onClick={() => {
                        const manualModel = search.trim();
                        const sameRuntime = browsingRuntimeId === runtimeId;
                        const manualUpdate = buildModelChangeUpdate({
                          provider: runtime?.provider ?? "",
                          model: manualModel,
                          thinkingLevel: sameRuntime ? thinkingLevel : "",
                          serviceTier: sameRuntime ? serviceTier : "",
                          catalog,
                        });
                        void select({
                          runtimeId: browsingRuntimeId,
                          model: manualModel,
                          thinkingLevel:
                            sameRuntime && manualUpdate.thinking_level !== ""
                              ? thinkingLevel
                              : "",
                        });
                      }}
                    >
                      {t(($) => $.pickers.model_custom_use, {
                        value: search.trim(),
                      })}
                    </button>
                  )}
                  {!modelsQuery.isLoading &&
                    !modelsQuery.isError &&
                    filtered.length === 0 &&
                    !canCreate && (
                      <p className="px-2 py-5 text-caption text-muted-foreground">
                        {t(($) => $.pickers.model_empty_with_dot)}
                      </p>
                    )}
                </>
              )}
            </div>
            <div
              aria-label={t(($) => $.model_selector.effort)}
              className="min-w-[10rem] overflow-y-auto border-l border-border/60 p-1.5"
            >
              <p className="px-2 py-1 text-micro text-muted-foreground">
                {t(($) => $.model_selector.effort)}
              </p>
              {!allowEffort && !browsingOption?.variants ? (
                <div className="px-2 py-5 text-caption text-muted-foreground">
                  <p>
                    {thinkingLevel ||
                      t(($) => $.model_selector.effort_inherited)}
                  </p>
                  {thinkingLevel ? (
                    <p className="mt-2">
                      {t(($) => $.model_selector.effort_inherited)}
                    </p>
                  ) : null}
                </div>
              ) : browsingModel && supported ? (
                <>
                  {(browsingOption?.variants
                    ? browsingOption.variants.map((variant) => ({
                        value: "",
                        label: nativeEffortLabel(variant),
                        model: variant.id,
                      }))
                    : allowEffort
                      ? levels.map((level) => ({
                          ...level,
                          model: browsingModel,
                        }))
                      : []
                  ).map((level) => {
                    const choice = {
                      ...currentChoice(level.value),
                      model: level.model,
                      serviceTier: allowSpeed
                        ? explicitServiceTier(
                            findModelCapabilityEntry(
                              models,
                              level.model,
                              runtime?.provider ?? "",
                            ),
                            browsingServiceTier,
                          )
                        : serviceTier,
                    };
                    return (
                      <div
                        key={`${level.model}:${level.value}`}
                        className={cn(
                          "flex items-center rounded-md",
                          browsingRuntimeId === runtimeId &&
                            level.model === model &&
                            thinkingLevel === level.value &&
                            "bg-accent text-accent-foreground font-medium ring-1 ring-inset ring-border",
                        )}
                      >
                        <button
                          type="button"
                          disabled={saving}
                          className={rowClass}
                          aria-pressed={
                            browsingRuntimeId === runtimeId &&
                            level.model === model &&
                            thinkingLevel === level.value
                          }
                          title={level.label}
                          onClick={() => void select(choice)}
                        >
                          <span className="min-w-0 flex-1 break-words">
                            {level.label}
                          </span>
                          {browsingRuntimeId === runtimeId &&
                            level.model === model &&
                            thinkingLevel === level.value && (
                              <Check aria-hidden className="size-4 shrink-0" />
                            )}
                        </button>
                        {entry &&
                          star(
                            choice,
                            combinationLabel(
                              browsingOption?.label ?? entry.label,
                              level.label,
                              choice,
                              findModelCapabilityEntry(
                                models,
                                level.model,
                                runtime?.provider ?? "",
                              ),
                            ),
                          )}
                      </div>
                    );
                  })}
                  {!levels.length && !browsingOption?.variants && (
                    <p className="px-2 py-3 text-micro text-muted-foreground">
                      {t(($) => $.model_selector.no_effort)}
                    </p>
                  )}
                </>
              ) : (
                <p className="px-2 py-5 text-caption text-muted-foreground">
                  {t(($) => $.model_selector.choose_model)}
                </p>
              )}
            </div>
            <ModelSpeedColumn
              tiers={entry?.service_tiers ?? []}
              supportsExplicitStandard={
                entry?.supports_explicit_standard_service_tier === true
              }
              value={allowSpeed ? browsingServiceTier : serviceTier}
              editable={allowSpeed}
              disabled={saving}
              onSelect={(nextTier) =>
                void select({ ...browsingChoice, serviceTier: nextTier })
              }
            />
          </div>
        )}
      </div>
    </div>
  );
}
