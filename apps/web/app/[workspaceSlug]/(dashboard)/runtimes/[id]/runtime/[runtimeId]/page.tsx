"use client";

import { use } from "react";
import { RuntimeSettingsPage } from "@cordy/views/runtimes";

export default function RuntimeSettingsRoute({
  params,
}: {
  params: Promise<{ id: string; runtimeId: string }>;
}) {
  const { id, runtimeId } = use(params);
  return <RuntimeSettingsPage machineId={id} runtimeId={runtimeId} />;
}
