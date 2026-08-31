"use client";

import type {
  AriaAttributes,
  ButtonHTMLAttributes,
  ComponentProps,
  CSSProperties,
  ElementType,
  ReactNode,
} from "react";
import { createContext, createElement, lazy, Suspense, useContext } from "react";
import { Input as PatchbayInput } from "@patchbay/ui/components/ui/input";
import { Switch as PatchbaySwitch } from "@patchbay/ui/components/ui/switch";

/**
 * The context is intentionally kept in this lightweight module. Embedded
 * settings tabs can use the Patchbay fallback without loading Lobe's antd
 * runtime; the standalone page imports the provider explicitly.
 */
export const LobeSettingsContext = createContext(false);

const LobeRuntimeSwitch = lazy(() =>
  import("./lobe-settings-runtime").then(({ LobeRuntimeSwitch: Switch }) => ({
    default: Switch,
  })),
);
const LobeRuntimeText = lazy(() =>
  import("./lobe-settings-runtime").then(({ LobeRuntimeText: Text }) => ({
    default: Text,
  })),
);
const LobeRuntimeBackButton = lazy(() =>
  import("./lobe-settings-runtime").then(({ LobeRuntimeBackButton: Button }) => ({
    default: Button,
  })),
);
const LobeRuntimeInput = lazy(() =>
  import("./lobe-settings-runtime").then(({ LobeRuntimeInput: Input }) => ({
    default: Input,
  })),
);

export function useLobeSettings() {
  return useContext(LobeSettingsContext);
}

export type SettingsSwitchProps = {
  autoFocus?: boolean;
  checked?: boolean;
  className?: string;
  defaultChecked?: boolean;
  disabled?: boolean;
  form?: string;
  id?: string;
  name?: string;
  onCheckedChange?: (checked: boolean) => void;
  readOnly?: boolean;
  required?: boolean;
  size?: "sm" | "default";
  style?: CSSProperties;
  tabIndex?: number;
  title?: string;
} & AriaAttributes & {
  [key: `data-${string}`]: string | number | boolean | undefined;
};

/**
 * Switch adapter used by settings tabs so embedded and standalone surfaces
 * share behavior while the standalone page uses Lobe's Base UI switch.
 */
export function SettingsSwitch({
  onCheckedChange,
  size,
  ...props
}: SettingsSwitchProps) {
  const useLobe = useLobeSettings();
  const fallback = (
    <PatchbaySwitch
      {...props}
      size={size}
      onCheckedChange={(checked) => onCheckedChange?.(checked)}
    />
  );

  if (!useLobe) return fallback;
  return (
    <Suspense fallback={fallback}>
      <LobeRuntimeSwitch
        {...props}
        size={size}
        onCheckedChange={onCheckedChange}
      />
    </Suspense>
  );
}

export type SettingsTextProps = {
  as?: ElementType;
  children?: ReactNode;
  className?: string;
  [key: string]: unknown;
};

/** Typography adapter that preserves the existing semantic element when the
 * Lobe provider is not present. */
export function SettingsText({ as = "span", children, ...props }: SettingsTextProps) {
  const useLobe = useLobeSettings();
  const fallback = createElement(as, props, children);
  if (!useLobe) return fallback;
  return (
    <Suspense fallback={fallback}>
      <LobeRuntimeText as={as} {...props}>
        {children}
      </LobeRuntimeText>
    </Suspense>
  );
}

export type SettingsBackButtonProps = Omit<
  ButtonHTMLAttributes<HTMLButtonElement>,
  "color"
>;

/** Lobe Button adapter for the standalone settings exit affordance. */
export function SettingsBackButton({ children, ...props }: SettingsBackButtonProps) {
  const useLobe = useLobeSettings();
  const fallback = <button {...props}>{children}</button>;
  if (!useLobe) return fallback;
  return (
    <Suspense fallback={fallback}>
      <LobeRuntimeBackButton {...props}>{children}</LobeRuntimeBackButton>
    </Suspense>
  );
}

export type SettingsInputProps = ComponentProps<typeof PatchbayInput>;

/** Lobe Input adapter for the common text controls in settings. */
export function SettingsInput(props: SettingsInputProps) {
  const useLobe = useLobeSettings();
  const fallback = <PatchbayInput {...props} />;
  if (!useLobe) return fallback;
  return (
    <Suspense fallback={fallback}>
      <LobeRuntimeInput {...props} />
    </Suspense>
  );
}
