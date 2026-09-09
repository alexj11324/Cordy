import { redirect } from "next/navigation";

export default async function LegacyRuntimesRedirect({
  params,
}: {
  params: Promise<{ workspaceSlug: string }>;
}) {
  const { workspaceSlug } = await params;
  redirect(`/${encodeURIComponent(workspaceSlug)}/devices`);
}
