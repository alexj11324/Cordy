# Standalone settings Lobe UI migration QA

## Evidence

- Source visual truth:
  - `/home/ubuntu/.codex/attachments/d97b40de-a66e-42e1-955a-ec6c0bb7b4cc/codex-clipboard-4097918c-f442-40ef-80e7-4163a7d9108d.png` — first-class settings page hierarchy.
  - `/home/ubuntu/.codex/attachments/bb4b54c5-6cbc-47d5-a1cc-d8a03ef55757/codex-clipboard-100a3464-f478-4e7f-8d7a-32f6187c0f0b.png` — existing sidebar typography, color, and density.
- Lobe references: [Chat Input Area](https://ui.lobehub.com/components/chat/chat-input-area), [Base UI Button](https://ui.lobehub.com/components/base-ui/button), and [Base UI Switch](https://ui.lobehub.com/components/base-ui/switch).
- The earlier `settings-lobe-ui.png` artifact was a temporary navigation fixture, not a production screenshot. It intentionally rendered only a reduced account list and mocked panel data, so it is not valid evidence for the settings page's content.
- Production browser evidence: `/home/ubuntu/.codex/visualizations/2026/08/31/01a055ff-b2d0-74b3-9134-63f819d5b070/settings-production-shell.png` (1686 × 986 CSS px, dark theme, Simplified Chinese, Profile selected). This screenshot mounts the real desktop `SettingsPage`, renders all 20 visible destinations, and shows the existing Profile panel content.
- Regression coverage: `packages/views/settings/components/settings-page.test.tsx` asserts that every existing destination remains in the standalone navigation.

## Findings

The sparse screenshot is rejected as implementation evidence: it came from a fixture that did not mount the production `SettingsPage`.

- `SettingsPage` still defines every existing account and workspace destination and maps each visible destination to its existing panel component. The standalone branch passes those same items to `LobeSettingsTabs`; the migration did not delete the right-hand settings panels.
- The standalone navigation now has regression coverage for all account, desktop, and workspace destinations, including API Tokens, Daemon, Updates, General, Repositories, GitHub, Integrations, Labs, Members, Labels, Issue Statuses, Properties, Quick Actions, and MCP. Chromium smoke verification found all 20 tabs and no console/page errors.
- The Lobe provider is explicitly synchronized with the app's `dark` class and `next-themes` value, so antd/Lobe tokens stay readable on the existing page-canvas palette.
- Sidebar width, selected/hover states, typography hierarchy, card borders, and content inset remain aligned with the existing sidebar tokens. Embedded settings continue to use the existing controls through the adapter fallback.
- Chinese text rendered correctly with the installed system Noto CJK fonts.

The old visual fixture must not be used to claim that the production right-hand content is sparse. The corrected implementation evidence and regression test preserve the original settings surface while retaining the Lobe UI migration.

## Final result

final result: passed
