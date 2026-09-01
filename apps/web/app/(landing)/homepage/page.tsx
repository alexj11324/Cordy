import type { Metadata } from "next";
import { PatchbayLanding } from "@/features/landing/components/patchbay-landing";

export const metadata: Metadata = {
  title: "Homepage",
  description:
    "Patchbay — open-source platform that turns coding agents into real teammates. Assign tasks, track progress, compound skills.",
  openGraph: {
    title: "Patchbay — Project Management for Human + Agent Teams",
    description:
      "Manage your human + agent workforce in one place.",
    url: "/homepage",
  },
  alternates: {
    canonical: "/homepage",
  },
};

export default function HomepagePage() {
  return <PatchbayLanding />;
}
