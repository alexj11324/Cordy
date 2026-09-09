"use client";

import { use } from "react";
import { RuntimeDetailPage } from "@orvilo/views/runtimes";

export default function DeviceDetailRoute({
  params,
}: {
  params: Promise<{ id: string }>;
}) {
  const { id } = use(params);
  return <RuntimeDetailPage runtimeId={id} />;
}
