function docsLocaleSegment(language?: string): string {
  if (language?.startsWith("zh")) return "/zh";
  return "";
}

export function daemonRuntimesDocsHref(language?: string): string {
  return `https://patchbay.aspectlylabs.com/docs${docsLocaleSegment(language)}/daemon-runtimes`;
}

export function customRuntimeDocsHref(language?: string): string {
  const base = daemonRuntimesDocsHref(language);
  if (language?.startsWith("zh")) {
    return `${base}#${encodeURIComponent("自定义运行时配置")}`;
  }
  return `${base}#custom-runtime-profiles`;
}
