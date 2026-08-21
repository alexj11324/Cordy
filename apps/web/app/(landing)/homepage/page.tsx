import type { Metadata } from "next";
import { CordyLanding } from "@/features/landing/components/cordy-landing";

export const metadata: Metadata = {
  title: "Homepage",
  description:
    "Cordy — open-source platform that turns coding agents into real teammates. Assign tasks, track progress, compound skills.",
  openGraph: {
    title: "Cordy — Project Management for Human + Agent Teams",
    description:
      "Manage your human + agent workforce in one place.",
    url: "/homepage",
  },
  alternates: {
    canonical: "/homepage",
  },
};

export default function HomepagePage() {
  return <CordyLanding />;
}
