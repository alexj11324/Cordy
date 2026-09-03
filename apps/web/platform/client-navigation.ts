"use client";

import { useRouter, useSearchParams } from "next/navigation";

export function useWebRouter() {
  return useRouter();
}

export function useWebSearchParams() {
  return useSearchParams();
}
