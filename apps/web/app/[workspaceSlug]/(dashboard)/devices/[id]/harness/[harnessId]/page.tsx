"use client";

import { use } from "react";
import { RuntimeSettingsPage } from "@orvilo/views/runtimes";

export default function HarnessSettingsRoute({
  params,
}: {
  params: Promise<{ id: string; harnessId: string }>;
}) {
  const { id, harnessId } = use(params);
  return <RuntimeSettingsPage machineId={id} runtimeId={harnessId} />;
}
