/**
 * Semantic avatar size scale. Avatar consumers pick a role-based tier instead
 * of a raw pixel number so the same "role" (inline chip, list row, section
 * header, hero) renders at one consistent size everywhere.
 *
 * Identity avatars have a 24px minimum so faces and fallback icons stay legible.
 * Legacy compact tiers are aliases of that minimum. The px values are
 * the single source of truth for any component that still needs the concrete
 * diameter (font scaling, overlap math, presence-dot thresholds).
 */
export type AvatarSize = "xs" | "sm" | "md" | "lg" | "xl" | "2xl";

/** Pixel diameter for each semantic avatar size. */
export const AVATAR_SIZE_PX: Record<AvatarSize, number> = {
  xs: 24,
  sm: 24,
  md: 24,
  lg: 32,
  xl: 40,
  "2xl": 56,
};

/** Default tier for avatars that don't specify a size. */
export const DEFAULT_AVATAR_SIZE: AvatarSize = "sm";
