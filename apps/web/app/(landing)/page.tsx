import type { Metadata } from "next";
import { CordyLanding } from "@/features/landing/components/cordy-landing";
import { RedirectIfAuthenticated } from "@/features/landing/components/redirect-if-authenticated";

export const metadata: Metadata = {
  title: {
    absolute: "Cordy — Project Management for Human + Agent Teams",
  },
  description:
    "Open-source platform that turns coding agents into real teammates. Assign tasks, track progress, compound skills.",
  openGraph: {
    title: "Cordy — Project Management for Human + Agent Teams",
    description:
      "Manage your human + agent workforce in one place.",
    url: "/",
  },
  alternates: {
    canonical: "/",
  },
};

export default function LandingPage() {
  return (
    <>
      <RedirectIfAuthenticated />
      <CordyLanding />
    </>
  );
}
