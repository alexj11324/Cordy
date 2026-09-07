export function slackDocsUrl(lang: string | undefined): string {
  const prefix = lang?.startsWith("zh") ? "/zh" : "";
  return `https://patchbay.aspectlylabs.com/docs${prefix}/slack-bot-integration`;
}
