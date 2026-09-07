"use client";

import { useState } from "react";
import { AlarmClock, Plus } from "lucide-react";
import { Button } from "@orvilo/ui/components/ui/button";
import {
  Card,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@orvilo/ui/components/ui/card";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@orvilo/ui/components/ui/tabs";
import { cn } from "@orvilo/ui/lib/utils";
import { useT } from "../../i18n";
import {
  AUTOMATION_TEMPLATES,
  TEMPLATE_CATEGORIES,
  TEMPLATE_CATEGORY_IDS,
  type AutomationTemplate,
  type TemplateCategoryId,
} from "./automation-templates";

export function AutomationTemplateGallery({
  onSelectTemplate,
  onStartBlank,
  persistent = false,
}: {
  onSelectTemplate: (template: AutomationTemplate) => void;
  onStartBlank: () => void;
  /** Keep recommendations available below an existing automation list. */
  persistent?: boolean;
}) {
  const { t } = useT("automations");
  const [category, setCategory] = useState<TemplateCategoryId>("popular");

  return (
    <div
      data-testid="automation-template-gallery"
      className={cn(
        "@container mx-auto flex w-full max-w-4xl flex-col px-5",
        persistent ? "py-5" : "py-10",
      )}
    >
      <div
        className={cn(
          persistent
            ? "mb-4 flex items-start justify-between gap-4"
            : "mb-8 flex flex-col items-center text-center",
        )}
      >
        {!persistent ? (
          <AlarmClock className="mb-3 size-10 text-faint-foreground" />
        ) : null}
        <div>
          <h2
            className={cn(
              "font-medium",
              persistent ? "text-body" : "text-title-sm",
            )}
          >
            {persistent
              ? t(($) => $.page.recommended.title)
              : t(($) => $.page.empty.title)}
          </h2>
          <p
            className={cn(
              "mt-1 text-caption text-muted-foreground",
              !persistent && "max-w-xl",
            )}
          >
            {persistent
              ? t(($) => $.page.recommended.hint)
              : t(($) => $.page.empty.hint)}
          </p>
        </div>
        {persistent ? (
          <Button size="sm" variant="outline" onClick={onStartBlank}>
            <Plus className="mr-1 size-3.5" />
            {t(($) => $.page.start_blank)}
          </Button>
        ) : null}
      </div>

      <Tabs
        value={category}
        onValueChange={(value) => {
          if (isTemplateCategoryId(value)) setCategory(value);
        }}
        className="gap-4"
      >
        <TabsList className="h-auto w-full flex-wrap justify-start gap-1 bg-transparent p-0 group-data-horizontal/tabs:h-auto">
          {TEMPLATE_CATEGORIES.map((cat) => (
            <TabsTrigger
              key={cat.id}
              value={cat.id}
              className={cn(
                "flex-none rounded-full border-transparent px-3 py-1.5 text-body font-medium",
                "data-active:bg-muted data-active:text-foreground data-active:shadow-none",
                "data-active:hover:bg-muted data-active:hover:text-foreground",
                "dark:data-active:border-transparent dark:data-active:bg-muted",
              )}
            >
              {t(($) => $.template_categories[cat.id])}
            </TabsTrigger>
          ))}
        </TabsList>

        {TEMPLATE_CATEGORIES.map((cat) => (
          <TabsContent key={cat.id} value={cat.id} className="mt-1">
            <div className="grid grid-cols-1 gap-4 @xl:grid-cols-2">
              {cat.templateIds.map((id) => (
                <TemplateCard
                  key={id}
                  template={AUTOMATION_TEMPLATES[id]}
                  onSelect={() => onSelectTemplate(AUTOMATION_TEMPLATES[id])}
                />
              ))}
            </div>
          </TabsContent>
        ))}
      </Tabs>

      {!persistent ? (
        <div className="mt-6 flex justify-center">
          <Button size="sm" variant="outline" onClick={onStartBlank}>
            <Plus className="mr-1 size-3.5" />
            {t(($) => $.page.start_blank)}
          </Button>
        </div>
      ) : null}
    </div>
  );
}

function TemplateCard({
  template,
  onSelect,
}: {
  template: AutomationTemplate;
  onSelect: () => void;
}) {
  const { t } = useT("automations");
  return (
    <Card size="sm" className="h-full gap-0 py-0 shadow-none">
      <button
        type="button"
        onClick={onSelect}
        className="flex h-full w-full flex-col rounded-[inherit] p-4 text-left transition-colors hover:bg-accent/40"
      >
        <CardHeader className="gap-1 p-0">
          <CardTitle className="text-body font-medium">
            {t(($) => $.templates[template.id].title)}
          </CardTitle>
          <CardDescription className="line-clamp-2 text-caption">
            {t(($) => $.templates[template.id].summary)}
          </CardDescription>
        </CardHeader>
      </button>
    </Card>
  );
}

function isTemplateCategoryId(value: unknown): value is TemplateCategoryId {
  return (
    typeof value === "string" &&
    (TEMPLATE_CATEGORY_IDS as readonly string[]).includes(value)
  );
}
