import { redirect } from "next/navigation";

export default async function LegacyRuntimeSettingsRedirect({
  params,
}: {
  params: Promise<{ workspaceSlug: string; id: string; runtimeId: string }>;
}) {
  const { workspaceSlug, id, runtimeId } = await params;
  redirect(`/${encodeURIComponent(workspaceSlug)}/devices/${encodeURIComponent(id)}/harness/${encodeURIComponent(runtimeId)}`);
}
