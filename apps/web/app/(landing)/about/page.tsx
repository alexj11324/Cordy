import type { Metadata } from "next";
import { AboutPageClient } from "@/features/landing/components/about-page-client";

export const metadata: Metadata = {
  title: "About",
  description:
    "Learn about Patchbay — multiplexed information and computing agent. An open-source project management platform for human + agent teams.",
  openGraph: {
    title: "About Patchbay",
    description:
      "The story behind Patchbay and why we're building project management for human + agent teams.",
    url: "/about",
  },
  alternates: {
    canonical: "/about",
  },
};

export default function AboutPage() {
  return <AboutPageClient />;
}
