# T3 Code model picker

The agent model selector adapts the provider rail, top favorites entry, selected
indicator, search layout, and model-row presentation from T3 Code's
`apps/web/src/components/chat/ModelPickerSidebar.tsx`, `ModelPickerContent.tsx`,
and `ModelListRow.tsx` at commit `223ff4490f764a74ff911589e97b9bbcd595fee8`.

Source: https://github.com/pingdotgg/t3code
License: MIT, Copyright (c) 2026 T3 Tools Inc. See `t3code-LICENSE`.

Orvilo uses its own runtime catalog, React Query, UI components, and persisted
favorites store. It adds a third column for catalog-supported thinking effort;
favorites identify the runtime, model, and effort together.
