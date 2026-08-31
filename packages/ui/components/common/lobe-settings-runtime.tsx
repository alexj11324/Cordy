"use client";

import type { ComponentProps, ReactNode } from "react";
import {
  default as LobeButton,
} from "@lobehub/ui/es/Button/Button";
import { default as LobeConfigProvider } from "@lobehub/ui/es/ConfigProvider/index";
import { default as LobeInput } from "@lobehub/ui/es/Input/Input";
import { default as LobeText } from "@lobehub/ui/es/Text/Text";
import { default as LobeThemeProvider } from "@lobehub/ui/es/ThemeProvider/ThemeProvider";
import {
  SwitchRoot as LobeSwitchRoot,
  SwitchThumb as LobeSwitchThumb,
} from "@lobehub/ui/es/base-ui/Switch/atoms";
import { motion } from "motion/react";
import {
  LobeSettingsContext,
  type SettingsBackButtonProps,
  type SettingsInputProps,
  type SettingsSwitchProps,
  type SettingsTextProps,
} from "./lobe-settings";
import { useTheme } from "./theme-provider";

export function LobeRuntimeProvider({ children }: { children: ReactNode }) {
  const { resolvedTheme } = useTheme();
  const appearance =
    resolvedTheme === "dark" ||
    (typeof document !== "undefined" &&
      document.documentElement.classList.contains("dark"))
      ? "dark"
      : "light";

  return (
    <LobeConfigProvider motion={motion}>
      <LobeThemeProvider
        appearance={appearance}
        themeMode={appearance}
        customTheme={{ neutralColor: "slate", primaryColor: "blue" }}
        enableCustomFonts={false}
        enableGlobalStyle={false}
        className="contents"
      >
        <LobeSettingsContext value>
          <div
            data-settings-ui="lobe-runtime"
            className="contents"
          >
            {children}
          </div>
        </LobeSettingsContext>
      </LobeThemeProvider>
    </LobeConfigProvider>
  );
}

export function LobeRuntimeSwitch({
  onCheckedChange,
  size,
  ...props
}: SettingsSwitchProps) {
  const lobeSize = size === "sm" ? "small" : "default";
  return (
    <LobeSwitchRoot
      {...props}
      size={lobeSize}
      onCheckedChange={(checked) => onCheckedChange?.(checked)}
    >
      <LobeSwitchThumb size={lobeSize} />
    </LobeSwitchRoot>
  );
}

export function LobeRuntimeText({ as = "span", children, ...props }: SettingsTextProps) {
  return (
    <LobeText as={as} {...props}>
      {children}
    </LobeText>
  );
}

export function LobeRuntimeBackButton({
  children,
  type: _type,
  ...props
}: SettingsBackButtonProps) {
  return (
    <LobeButton type="text" size="middle" {...props}>
      {children}
    </LobeButton>
  );
}

export function LobeRuntimeInput(props: SettingsInputProps) {
  const { ref: _ref, size: _size, ...lobeProps } = props;
  return <LobeInput {...lobeProps} />;
}

export type LobeRuntimeInputProps = ComponentProps<typeof LobeInput>;
