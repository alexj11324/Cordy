import { redirect } from "next/navigation";

/**
 * The per-Harness details surface was removed. Keep already-open links inside
 * the workspace and land them on the device inventory instead.
 */
export default async function HarnessSettingsRedirect({
  params,
}: {
  params: Promise<{ workspaceSlug: string; id: string; harnessId: string }>;
}) {
  const { workspaceSlug, id } = await params;
  redirect(
    `/${encodeURIComponent(workspaceSlug)}/devices/${encodeURIComponent(id)}`,
  );
}
