import { useEffect } from "react";
import { currentPath, useNavigation } from "@orvilo/views/navigation";

/** Replace the tab session; its coordinator owns the corresponding router move. */
export function LegacyRuntimesRedirect() {
  const navigation = useNavigation();
  const url = currentPath(navigation);
  const next = url
    .replace(/^(\/[^/]+)\/runtimes(?=\/|[?#]|$)/, "$1/devices")
    .replace(/^(\/[^/]+\/devices\/[^/]+)\/runtime\//, "$1/harness/");

  useEffect(() => {
    if (next !== url) navigation.replace(next);
  }, [navigation, next, url]);

  return null;
}
