"use client";

import { use } from "react";
import { RuntimeDetailPage } from "@patchbay/views/runtimes";

export default function RuntimeDetailRoute({
  params,
}: {
  params: Promise<{ id: string }>;
}) {
  const { id } = use(params);
  return <RuntimeDetailPage runtimeId={id} />;
}
