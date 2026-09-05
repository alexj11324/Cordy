// Share one explicit override between the dev listener and browser tests.
// strictPort deliberately fails rather than attaching to an unexpected server.
export const previewPort = Number(process.env.CARD_PREVIEW_PORT ?? "5188");
if (!Number.isInteger(previewPort) || previewPort < 1024 || previewPort > 65535) {
  throw new Error("CARD_PREVIEW_PORT must be an integer between 1024 and 65535");
}
export const previewUrl = `http://127.0.0.1:${previewPort}`;
