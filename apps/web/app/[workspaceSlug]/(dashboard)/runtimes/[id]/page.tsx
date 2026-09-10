import { redirect } from "next/navigation";

export default async function LegacyRuntimeDetailRedirect({
  params,
}: {
  params: Promise<{ workspaceSlug: string; id: string }>;
}) {
  const { workspaceSlug, id } = await params;
  redirect(`/${encodeURIComponent(workspaceSlug)}/devices/${encodeURIComponent(id)}`);
}
