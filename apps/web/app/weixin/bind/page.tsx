"use client";

import { Suspense } from "react";
import { useSearchParams } from "next/navigation";
import { WeixinBindPage } from "@patchbay/views/weixin";

function WeixinBindPageContent() {
  const searchParams = useSearchParams();
  return <WeixinBindPage token={searchParams.get("token")} />;
}

export default function Page() {
  return (
    <Suspense fallback={null}>
      <WeixinBindPageContent />
    </Suspense>
  );
}
