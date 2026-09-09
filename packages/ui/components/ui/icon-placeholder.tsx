import type { LucideIcon, LucideProps } from "lucide-react"
import * as LucideIcons from "lucide-react"

/**
 * ReUI blocks emit <IconPlaceholder lucide="SearchIcon" ... /> from the
 * create-kit. This maps the lucide export name onto lucide-react so the
 * block JSX can stay unmodified.
 */
export function IconPlaceholder({
  lucide,
  tabler: _tabler,
  hugeicons: _hugeicons,
  phosphor: _phosphor,
  remixicon: _remixicon,
  ...props
}: LucideProps & {
  lucide?: string
  tabler?: string
  hugeicons?: string
  phosphor?: string
  remixicon?: string
}) {
  const icons = LucideIcons as unknown as Record<string, LucideIcon>
  const Icon = (lucide && icons[lucide]) || LucideIcons.Circle
  return <Icon {...props} />
}
