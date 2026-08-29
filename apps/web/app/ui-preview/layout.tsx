import { notFound } from "next/navigation";

export default function UiPreviewLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  if (process.env.NODE_ENV !== "development") notFound();
  return <div className="h-full min-h-dvh">{children}</div>;
}
