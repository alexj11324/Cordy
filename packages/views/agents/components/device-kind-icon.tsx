import { Laptop, Monitor, Terminal } from "lucide-react";
import type { DeviceKind } from "@orvilo/core/runtimes";

export function DeviceKindIcon({
  kind,
  className = "size-4",
}: {
  kind: DeviceKind;
  className?: string;
}) {
  if (kind === "terminal") {
    return (
      <Terminal aria-hidden="true" className={className} data-kind="terminal" />
    );
  }
  if (kind === "laptop") {
    return (
      <Laptop aria-hidden="true" className={className} data-kind="laptop" />
    );
  }
  return (
    <Monitor aria-hidden="true" className={className} data-kind="desktop" />
  );
}
