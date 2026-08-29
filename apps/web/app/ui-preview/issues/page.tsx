import { notFound } from "next/navigation";
import { PreviewIssuesBoard } from "./preview-issues-board";

export default function UiPreviewIssuesPage() {
  if (process.env.NODE_ENV !== "development") notFound();
  return <PreviewIssuesBoard />;
}
