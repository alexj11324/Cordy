"use client";

import { type ReactNode } from "react";
import {
  Badge,
  type BadgeProps,
} from "@orvilo/ui/components/reui/badge";
import { UserRound } from "lucide-react";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@orvilo/ui/components/reui/card";
import {
  Item,
  ItemContent,
  ItemDescription,
  ItemMedia,
  ItemTitle,
} from "@orvilo/ui/components/reui/item";
import { Avatar, AvatarFallback, AvatarImage } from "@orvilo/ui/components/ui/avatar";
import {
  Progress,
  ProgressLabel,
} from "@orvilo/ui/components/ui/progress";
import { cn } from "@orvilo/ui/lib/utils";

/**
 * Atlas DealCard from Downloads/atlas-v1.1.0
 * (`features/deals/deal-pipeline.tsx`). JSX, class names, icons, badge,
 * progress, owner, and card interaction stay on the template's tree. Product
 * code maps agent values into these slots without changing its geometry.
 */

export type DealPerson = {
  id: string;
  name: string;
  title?: string;
  avatar: string;
};

export type AtlasDealCardStatus = {
  label: string;
  variant: NonNullable<BadgeProps["variant"]>;
  dotClass: string;
};

export type AtlasDealCardOpportunity = {
  id: string;
  account: string;
  logo: ReactNode;
  status: AtlasDealCardStatus;
  accessIcon: ReactNode;
  accessLabel: string;
  accessText: string;
  deviceIcon: ReactNode;
  deviceLabel: string;
  deviceText: string;
  concurrencyLabel: string;
  concurrencyText: string;
  concurrency: number;
  concurrencyProgressClass: string;
  concurrencyCaption: string;
  owner: DealPerson;
};

/** Shared outer height for the gallery card and its create-card sibling. */
export const ATLAS_DEAL_CARD_SIZE_CLASS = "h-[13.75rem]";

function DealSnapshotItem({
  icon,
  label,
  value,
}: {
  icon: ReactNode;
  label: string;
  value: string;
}) {
  return (
    <Item variant="outline" size="sm" className="min-w-0 flex-nowrap gap-2">
      <ItemMedia
        variant="icon"
        className="size-auto self-center text-muted-foreground"
      >
        {icon}
      </ItemMedia>
      <ItemContent className="min-w-0 gap-0">
        <ItemDescription className="truncate text-[0.6875rem] leading-3.5">
          {label}
        </ItemDescription>
        <ItemTitle
          className="truncate text-sm leading-5 font-medium tabular-nums"
          title={value}
        >
          {value}
        </ItemTitle>
      </ItemContent>
    </Item>
  );
}

function DealSnapshot({ opportunity }: { opportunity: AtlasDealCardOpportunity }) {
  return (
    <div className="grid grid-cols-2 gap-2">
      <DealSnapshotItem
        label={opportunity.accessLabel}
        value={opportunity.accessText}
        icon={opportunity.accessIcon}
      />
      <DealSnapshotItem
        label={opportunity.deviceLabel}
        value={opportunity.deviceText}
        icon={opportunity.deviceIcon}
      />
    </div>
  );
}

function ConcurrencyProgress({
  opportunity,
}: {
  opportunity: AtlasDealCardOpportunity;
}) {
  return (
    <div className="flex flex-col gap-2.5">
      <div className="flex items-center justify-between gap-2 text-sm">
        <span className="text-muted-foreground">{opportunity.concurrencyLabel}</span>
        <span className="font-medium tabular-nums">
          {opportunity.concurrencyText}
        </span>
      </div>
      <Progress
        aria-label={opportunity.concurrencyCaption}
        aria-valuetext={opportunity.concurrencyCaption}
        value={opportunity.concurrency}
        className={cn(
          "gap-0 **:data-[slot=progress-indicator]:rounded-full **:data-[slot=progress-track]:h-1.5 **:data-[slot=progress-track]:rounded-full **:data-[slot=progress-track]:bg-muted",
          opportunity.concurrencyProgressClass,
        )}
      >
        <ProgressLabel className="sr-only">
          {opportunity.concurrencyCaption}
        </ProgressLabel>
      </Progress>
    </div>
  );
}

function DealStatusBadge({ status }: { status: AtlasDealCardStatus }) {
  return (
    <Badge
      aria-label={status.label}
      className="max-w-full gap-1.5"
      size="sm"
      variant={status.variant}
    >
      <span
        aria-hidden="true"
        className={cn("size-1.5 shrink-0 rounded-full", status.dotClass)}
      />
      <span className="truncate">{status.label}</span>
    </Badge>
  );
}

function DealOwner({ opportunity }: { opportunity: AtlasDealCardOpportunity }) {
  return (
    <div className="flex min-w-0 items-center gap-2">
      <Avatar size="sm" className="size-7">
        {opportunity.owner.avatar ? (
          <AvatarImage
            src={opportunity.owner.avatar}
            alt={opportunity.owner.name}
          />
        ) : null}
        <AvatarFallback className="bg-primary/10 text-primary">
          <UserRound aria-hidden="true" className="size-3.5" />
        </AvatarFallback>
      </Avatar>
      <div className="min-w-0">
        <div className="truncate text-sm font-medium">
          {opportunity.owner.name}
        </div>
        {opportunity.owner.title && (
          <div className="truncate text-xs text-muted-foreground">
            {opportunity.owner.title}
          </div>
        )}
      </div>
    </div>
  );
}

export function AtlasDealCard({
  opportunity,
  isOverlay,
  onOpen,
}: {
  opportunity: AtlasDealCardOpportunity;
  isOverlay?: boolean;
  onOpen?: () => void;
}) {
  const card = (
    <Card
      size="sm"
      className={cn(
        ATLAS_DEAL_CARD_SIZE_CLASS,
        "w-full gap-0 bg-card p-0 shadow-xs transition-[border-color,box-shadow] hover:border-foreground/20 hover:shadow-sm",
        isOverlay && "shadow-lg",
      )}
    >
      <CardHeader className="grid min-h-5 min-w-0 grid-cols-[1.5rem_minmax(0,1fr)] items-center gap-x-2.5 px-3 pt-3 pb-0">
        <Item
          render={<span />}
          className="flex h-5 w-6 items-center justify-center border-0 p-0 [&_svg]:max-h-5 [&_svg]:max-w-6"
        >
          <ItemMedia variant="icon" className="size-auto">
            {opportunity.logo}
          </ItemMedia>
        </Item>
        <div className="flex min-w-0 items-center justify-between gap-2">
          <CardTitle
            className="min-w-0 truncate text-sm leading-5"
            title={opportunity.account}
          >
            <button
              type="button"
              disabled={!onOpen}
              className="w-full truncate text-left outline-none focus-visible:underline disabled:pointer-events-none"
              onClick={(event) => {
                event.stopPropagation();
                onOpen?.();
              }}
            >
              {opportunity.account}
            </button>
          </CardTitle>
          <DealStatusBadge status={opportunity.status} />
        </div>
      </CardHeader>

      <CardContent className="flex min-h-[10.75rem] flex-col gap-3 px-3 pt-3 pb-3">
        <DealSnapshot opportunity={opportunity} />

        <ConcurrencyProgress opportunity={opportunity} />

        <div className="mt-auto flex min-w-0 items-center gap-3">
          <DealOwner opportunity={opportunity} />
        </div>
      </CardContent>
    </Card>
  );

  return (
    <div
      className="block"
      data-testid={`agent-card-${opportunity.id}`}
      onClick={onOpen}
    >
      {card}
    </div>
  );
}
