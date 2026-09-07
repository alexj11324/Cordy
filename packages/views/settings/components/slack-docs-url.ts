export function slackDocsUrl(lang: string | undefined): string {
  const prefix = lang?.startsWith("zh") ? "/zh" : "";
  return `https://orvilo.aspectlylabs.com/docs${prefix}/slack-bot-integration`;
}
