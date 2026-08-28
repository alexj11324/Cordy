import type { Metadata } from "next";
import { AboutPageClient } from "@/features/landing/components/about-page-client";

export const metadata: Metadata = {
  title: "About",
  description:
    "Learn about Patchbay, the open-source coordination surface for human + agent teams.",
  openGraph: {
    title: "About Patchbay",
    description:
      "The story behind Patchbay and why we're building visible, human-directed coordination for agent teams.",
    url: "/about",
  },
  alternates: {
    canonical: "/about",
  },
};

export default function AboutPage() {
  return <AboutPageClient />;
}
