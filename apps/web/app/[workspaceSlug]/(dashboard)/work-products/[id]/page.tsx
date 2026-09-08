"use client";

import { use } from "react";
import { WorkProductDetailPage } from "@orvilo/views/work-products";

export default function Page({
  params,
}: {
  params: Promise<{ id: string }>;
}) {
  const { id } = use(params);
  return <WorkProductDetailPage id={id} />;
}
