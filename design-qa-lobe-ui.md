# Standalone settings Lobe UI migration QA

## Evidence

- Source visual truth:
  - `/home/ubuntu/.codex/attachments/d97b40de-a66e-42e1-955a-ec6c0bb7b4cc/codex-clipboard-4097918c-f442-40ef-80e7-4163a7d9108d.png` — first-class settings page hierarchy.
  - `/home/ubuntu/.codex/attachments/bb4b54c5-6cbc-47d5-a1cc-d8a03ef55757/codex-clipboard-100a3464-f478-4e7f-8d7a-32f6187c0f0b.png` — existing sidebar typography, color, and density.
- Lobe references: [Chat Input Area](https://ui.lobehub.com/components/chat/chat-input-area), [Base UI Button](https://ui.lobehub.com/components/base-ui/button), and [Base UI Switch](https://ui.lobehub.com/components/base-ui/switch).
- Implementation screenshot: `/home/ubuntu/.codex/visualizations/2026/08/31/01a055ff-b2d0-74b3-9134-63f819d5b070/settings-lobe-ui.png`
- Combined source/prototype comparison: `/home/ubuntu/.codex/visualizations/2026/08/31/01a055ff-b2d0-74b3-9134-63f819d5b070/settings-lobe-ui-comparison.png`
- Viewport: `1686 × 986` CSS px, `deviceScaleFactor: 2`; both images were normalized to the same CSS size before comparison.
- State: dark theme, Simplified Chinese, Profile selected, realistic local profile and permission data.

## Findings

No actionable P0, P1, or P2 findings remain.

- The standalone surface now uses Lobe Base UI Tabs for grouped account/workspace navigation and Lobe-themed Text, Input, Switch, and back-button adapters while keeping the existing settings semantics.
- The Lobe provider is explicitly synchronized with the app's `dark` class and `next-themes` value, so antd/Lobe tokens stay readable on the existing page-canvas palette.
- Sidebar width, selected/hover states, typography hierarchy, card borders, and content inset remain aligned with the existing sidebar tokens. Embedded settings continue to use the existing controls through the adapter fallback.
- Chromium interaction QA switched from Profile to Preferences, toggled an uncontrolled switch (`true` → `false`), restored focus to the return button, and recorded no console or page errors. The runtime provider reported the dark appearance.
- Chinese text rendered correctly with the installed system Noto CJK fonts.

## Final result

final result: passed
