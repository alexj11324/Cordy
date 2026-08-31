"use client";

import { use } from "react";
import { AutomationDetailPage } from "@patchbay/views/automations/components";

export default function Page({
  params,
}: {
  params: Promise<{ id: string }>;
}) {
  const { id } = use(params);
  return <AutomationDetailPage automationId={id} />;
}
