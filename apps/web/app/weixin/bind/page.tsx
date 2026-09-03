"use client";

import { Suspense } from "react";
import { useSearchParams } from "next/navigation";
import { WeixinBindPage } from "@patchbay/views/weixin";
function Content() {
  return <WeixinBindPage token={useSearchParams().get("token")} />;
}

export default function Page() {
  return (
    <Suspense fallback={null}>
      <Content />
    </Suspense>
  );
}
