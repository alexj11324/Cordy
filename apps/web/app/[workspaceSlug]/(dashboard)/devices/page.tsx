import { RuntimesPage } from "@orvilo/views/runtimes";

const cloudRuntimeEnabled =
  process.env.NEXT_PUBLIC_ENABLE_CLOUD_RUNTIME === "true";

export default function DevicesRoute() {
  return <RuntimesPage cloudRuntimeEnabled={cloudRuntimeEnabled} />;
}
