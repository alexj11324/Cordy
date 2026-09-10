import { redirect } from "next/navigation";

export default async function LegacyRuntimeSettingsRedirect({
  params,
}: {
  params: Promise<{ workspaceSlug: string; id: string; runtimeId: string }>;
}) {
  const { workspaceSlug, id } = await params;
  redirect(`/${encodeURIComponent(workspaceSlug)}/devices/${encodeURIComponent(id)}`);
}
