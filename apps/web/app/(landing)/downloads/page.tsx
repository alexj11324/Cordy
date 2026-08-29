import { redirect } from "next/navigation";

/** Stable plural alias for the existing release-backed download page. */
export default function DownloadsPage() {
  redirect("/download");
}
