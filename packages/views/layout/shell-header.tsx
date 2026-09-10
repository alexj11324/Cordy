"use client";

import {
  createContext,
  useCallback,
  useContext,
  useLayoutEffect,
  useMemo,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { cn } from "@orvilo/ui/lib/utils";

type ShellHeaderContextValue = {
  slot: HTMLElement | null;
  setSlot: (node: HTMLElement | null) => void;
  breadcrumbSlot: HTMLElement | null;
  setBreadcrumbSlot: (node: HTMLElement | null) => void;
  hasBreadcrumb: boolean;
  setHasBreadcrumb: (active: boolean) => void;
};

const ShellHeaderContext = createContext<ShellHeaderContextValue | null>(null);

/**
 * Lets a page park its primary actions in the shell titlebar — the same row
 * as the workspace breadcrumb — instead of repeating the page title one
 * row below.
 */
export function ShellHeaderProvider({ children }: { children: ReactNode }) {
  const [slot, setSlot] = useState<HTMLElement | null>(null);
  const [breadcrumbSlot, setBreadcrumbSlot] = useState<HTMLElement | null>(null);
  const [hasBreadcrumb, setHasBreadcrumb] = useState(false);
  const value = useMemo(
    () => ({
      slot,
      setSlot,
      breadcrumbSlot,
      setBreadcrumbSlot,
      hasBreadcrumb,
      setHasBreadcrumb,
    }),
    [slot, breadcrumbSlot, hasBreadcrumb],
  );
  return (
    <ShellHeaderContext.Provider value={value}>
      {children}
    </ShellHeaderContext.Provider>
  );
}

/** The route title is the fallback until a detail page supplies its full trail. */
export function ShellHeaderBreadcrumbSlot({ fallback }: { fallback: ReactNode }) {
  const ctx = useContext(ShellHeaderContext);
  if (!ctx) return <>{fallback}</>;
  return (
    <div
      ref={ctx.setBreadcrumbSlot}
      data-slot="shell-header-breadcrumb"
      className="flex min-w-0 items-center gap-1.5"
    >
      {!ctx.hasBreadcrumb ? fallback : null}
    </div>
  );
}

export function useShellHeaderBreadcrumbSlot() {
  return useContext(ShellHeaderContext)?.breadcrumbSlot ?? null;
}

export function ShellHeaderBreadcrumb({ children }: { children: ReactNode }) {
  const ctx = useContext(ShellHeaderContext);
  const setHasBreadcrumb = ctx?.setHasBreadcrumb;
  useLayoutEffect(() => {
    setHasBreadcrumb?.(true);
    return () => setHasBreadcrumb?.(false);
  }, [setHasBreadcrumb]);
  return ctx?.breadcrumbSlot ? createPortal(children, ctx.breadcrumbSlot) : null;
}

export function ShellHeaderActionsSlot({
  className,
  style,
}: {
  className?: string;
  style?: CSSProperties;
}) {
  const ctx = useContext(ShellHeaderContext);
  const setSlot = ctx?.setSlot;
  const ref = useCallback(
    (node: HTMLDivElement | null) => {
      setSlot?.(node);
    },
    [setSlot],
  );

  if (!ctx) return null;

  return (
    <div
      ref={ref}
      data-slot="shell-header-actions"
      className={cn("flex shrink-0 items-center gap-2", className)}
      style={style}
    />
  );
}

/**
 * Page-level primary actions. Portals into the shell titlebar when the
 * dashboard/desktop chrome is mounted; renders in place otherwise so a
 * page test does not have to mount the whole shell.
 */
export function ShellHeaderActions({ children }: { children: ReactNode }) {
  const ctx = useContext(ShellHeaderContext);
  if (!ctx) return <>{children}</>;
  if (!ctx.slot) return null;
  return createPortal(children, ctx.slot);
}
