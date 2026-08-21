"use client";

import { use } from "react";
import { AiBuilderSessionPage } from "@cordy/views/agents";

export default function NewAgentAiSessionRoute({
  params,
}: {
  params: Promise<{ sessionId: string }>;
}) {
  const { sessionId } = use(params);
  return <AiBuilderSessionPage sessionId={sessionId} />;
}
