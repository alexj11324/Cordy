# 聊天面板 LobeHub 化改造 — 实施记录

> 路线：全量接入 `@lobehub/ui` + `@lobehub/editor` 真组件。
> 状态：**Phase 0 / 1 / 2 已完成并验证**；Phase 3 经评估需决策；Phase 4 部分已具备。

---

## 一、完成情况总览

| Phase | 内容 | 状态 |
|-------|------|------|
| 0 | 依赖接入 + 风险验证 | ✅ 完成 |
| 1 | 主题桥接（Orvilo token → antd） | ✅ 完成 |
| 2 | 消息列表换成 LobeHub `ChatItem` | ✅ 完成 |
| 3 | Composer 换成 LobeHub 编辑器 | ⛔ **评估后需决策**（见 §5） |
| 4 | 收尾（reduced-motion / 清理 / 回归） | ✅ 大部分（见 §6） |

**验证结果**（Electron 路径，符合项目 CLAUDE.md 的 Verification 规则）：
- `pnpm --filter @orvilo/desktop typecheck` — node / web / cards 三项**全过**
- `pnpm --filter @orvilo/desktop build` — **通过**，`✓ built in 10.09s`
- `packages/views` 全量测试 — **5158 passed / 3 failed**（优于改动前基线 5147 passed / 7 failed；剩余 3 个是既有的 billing 日期测试，与本次无关）

---

## 二、Phase 0：发现的三个硬约束

### 2.1 OKLCH 与 antd 不兼容（关键）

`@ant-design/fast-color@3.0.1` **无法解析 `oklch()`** —— 实测所有 oklch 值都被解析成 `#000000`：

```
oklch(0.55 0.16 255) -> hex=#000000   ← 蓝色变黑色
#3b82f6              -> hex=#3b82f6   ← hex 正常
```

而 Orvilo 的设计 token **全部是 oklch**（`packages/ui/styles/tokens.css`）。若直接把 token 交给 antd，整个聊天面板会变成黑白的。

**解法**：实现 OKLCH → sRGB 转换（`chat/lobe/oklch.ts`），数学经标准值验证：
- `oklch(1 0 0)` → `#ffffff` ✅
- `oklch(0 0 0)` → `#000000` ✅
- `oklch(0.6279 0.2577 29.23)` → `#ff0000` ✅（CSS Color 4 规范的红）
- `oklch(0.55 0.16 255)` → `#2171cc`（品牌蓝）

### 2.2 空 token 会让整个面板崩溃

`@lobehub/ui` 的 `ThemeProvider` 内部用 `polished.mix()` 派生额外 token（`customToken.mjs:59`）。**空字符串进到颜色运算会抛错**，整个面板起不来。

**解法**：`createDomTokenReader()` 对读不到的 token 回退到静态默认值。这不是测试便利，是真实健壮性要求。

### 2.3 pnpm 10 拦截了 Lexical 补丁

`@lobehub/editor` 的 `postinstall` 会给 `lexical@0.42.0` 打哈希校验过的补丁。pnpm 10 出于供应链安全默认拦截 `postinstall`，且**不再读取 package.json 的 `pnpm` 字段**，所以白名单必须写进 `pnpm-workspace.yaml`：

```yaml
onlyBuiltDependencies:
  - '@lobehub/editor'
```

验证：patch 后 `Lexical.dev.js` 哈希 = `7c81a978...`，与脚本预期值精确匹配。

### 2.4 样式隔离：CSS Cascade Layers 兜住了

实测把 Tailwind 对照组放在 LobeHub 组件的**前后**，逐个属性对比计算样式：

| 属性 | LobeHub 挂载前 | 挂载后 |
|------|---------------|--------|
| muted 文字色 | `oklch(0.556 0 0)` | `oklch(0.556 0 0)` |
| 文字字号 | `14px` | `14px` |
| 按钮背景 | `oklch(0.205 0 0)` | `oklch(0.205 0 0)` |
| 按钮圆角 / 内边距 | `8px` / `8px` | `8px` / `8px` |

**逐字段完全相同，零污染。** 原因：LobeHub 把样式全部放进 `@layer lobe-base` / `@layer lobe-popup`，而 `@layer` 内的规则优先级**低于**未分层样式。框架本身已经做对了，不需要隔离 hack。

---

## 三、Phase 1：主题桥接

**产物**：`packages/views/chat/lobe/`

| 文件 | 职责 |
|------|------|
| `oklch.ts` | OKLCH → sRGB hex，纯函数 |
| `lobe-tokens.ts` | Orvilo token → antd token 映射，纯函数 |
| `lobe-theme-bridge.tsx` | 读 DOM token、订阅主题切换、挂 `StyleProvider` + `ThemeProvider` |

三个关键决策（都写进了代码注释）：

1. **`--brand` 驱动 `colorPrimary`，不是 `--primary`**。Orvilo 的 `--primary` 是 Pulse 体系的近黑中性色，用它会让聊天面板的强调色、链接、焦点环全部失色。

2. **CSS 变量命名空间用 `orvilo-lobe`**，不复用 LobeHub 的 `lobe-vars`。已验证 `@lobehub/ui` 零处硬编码引用该变量名，完全走 `cssVar` API，所以命名空间是我们的。

3. **`className="contents"`**。`ThemeProvider` 会渲染真实 `<div>`，而调用方把它挂在 flex 列里（消息列表夹在滚动容器和输入框之间），多出的盒子会分走布局。`display: contents` 去掉盒子但保留 CSS 变量继承。

4. **deep import `@lobehub/ui/es/ThemeProvider/index`**（是 default export）。用包根会把整个库拉进模块图 —— 在 Vitest 下会求值 `SortableList`，它需要 `defaultDropAnimationSideEffects`，而 7 个 issue-board 测试套件 mock 的 `@dnd-kit/core` 没提供这个导出，直接打挂。deep import 同时砍掉一大块 renderer bundle。

---

## 四、Phase 2：消息列表

**最重要的发现**：`ChatItemProps` 明确 `Omit<FlexboxProps, 'children'>` —— **它不接受 children**。而 `message?: ReactNode` 看着是入口，内部却被 `String()` 强制转换后交给 markdown 编辑器（`MessageContent.mjs`）：

```js
value: message ? String(message) : ""
```

传 ReactNode 过去会渲染成 `"[object Object]"`，内容全丢 —— 这正是最初 18 个测试失败的根因。

**正确接入口是 `renderMessage`**：它的返回值**整体替换**那个编辑器节点，让我们自己的富内容原样通过。

**改动**：`chat-message-list.tsx` 仅 **44 行插入 / 33 行删除** —— 把 `AIMessage`/`AIMessageContent`（AI Elements 的无样式 div）换成 `LobeMessageItem`，**内部所有业务逻辑原封不动**：

- 引用（citations）的 UTF-8 字节范围解析
- 任务时间线（`TimelineView` / 工具调用 / 思考）
- 快速操作、入门卡片、用量统计、失败气泡

这一切保留，因为它们与消息外壳无关。**换的是壳，不是内容。**

**得到的动效**（LobeHub 组件内置，无需自己写）：
- hover 揭示操作栏，含 `:has([data-popup-open])` 锁定 —— 鼠标移进下拉菜单时操作栏不会消失（自己写极易漏）
- `ResizeObserver` 自适应布局（内容宽则转纵向）
- 气泡样式、头像、标题、时间戳

---

## 五、Phase 3：ContentEditor 全面迁移到 LobeHub（用户已拍板：14 处一次到位）

### 5.1 真实范围（修正了先前的误判）

先前记的"12+ 处"是估的。精确清点后，`<ContentEditor>` 的**生产调用点为 14 个文件**：

| 层 | 文件 | 性质 |
|---|---|---|
| **A** | `chat/components/chat-input.tsx` | 聊天输入框 |
| **B** | `issues/components/comment-input.tsx`、`reply-input.tsx`、`comment-card.tsx`（编辑态）、`modals/create-issue.tsx`、`modals/feedback.tsx`、`modals/quick-create-issue.tsx` | 社交/表单输入框，走 `useComposerSubmit` 契约 |
| **C** | `issues/components/issue-detail.tsx`、`projects/components/project-detail.tsx`、`modals/create-project.tsx`、`teams/components/team-detail-page.tsx`、`agents/components/instructions-editor.tsx`、`automations/components/automation-edit-dialog.tsx`、`automation-settings-sections.tsx` | 长文档 / 配置编辑面 |

**误判的成因**（值得记住）：我先用"`useComposerSubmit` 的消费者"推断改造面，得到 7 处。但那只覆盖**走统一发送契约**的输入框；`ContentEditor` 在另外 7 个文件里被**直接使用**，不走那条路径，所以不出现在 hook 的搜索结果里。
→ **不能用"谁调用某个 hook"推断"谁使用某个组件"。**

### 5.2 成本（必须正视）

`packages/views/editor/` 共 **15,104 行**：

```
content-editor.tsx        1011   ← 待替换的内核
bubble-menu.tsx            706
attachment.tsx             554
attachment-preview-modal   974
use-coordinated-uploads    522
测试合计                  7740   ← 关键：这是行为契约的载体
```

**换 UI 内核的成本不在"画新界面"，而在把 7,740 行测试所断言的行为，逐条在 Lexical 上重新证明。**

### 5.3 接缝选择：改写 `content-editor.tsx` 内部，保持对外契约不变

不逐个改 14 个文件（那是 14 份风险），而是**让组件本身当接缝**：

- `ContentEditorRef` 的 15 个方法**签名保持不变**
- `ContentEditorProps` 的 props **保持不变**
- 只把内部 TipTap 实现换成 LobeHub 实现
- 14 个调用点**零改动**

旧 TipTap 实现随测试通过后删除（git 历史留存）。符合 CLAUDE.md"不保留双路径"。

### 5.4 已找到的对等物（这是可行的关键）

LobeHub 编辑器**独立演化出了和我们相同的上传状态机**：

| 我们的 `ContentEditorRef` | LobeHub 对等物 |
|---|---|
| `insertUploadPlaceholder({uploadId, filename, size})` | `$createFileNode(name, size)`，`status='pending'` |
| `settleUploadPlaceholder(id, result)` | `FileNode.setUploaded(url)` / `.setError(msg)` |
| `hasActiveUploads()` | 扫描 `FileNode` 中 `status === 'pending'` 者 |
| `uploadFile(file)` | `IUploadService.uploadFile(file, from, range)` |
| `insertMarkdownAtEnd(md)` | 节点插入命令 |
| 跨实例接管（MUL-5181） | `FileNode.exportJSON` / `importJSON` 可序列化 |

`FileNode` 的 `status: 'pending' | 'uploaded' | 'error'` 与我们的占位符三态**逐项对应**。
注册方式：`<Editor plugins={[[ReactFilePlugin, { handleUpload: file => Promise<{url}> }]]} />`。

→ **MUL-5181 的上传协调机制可整体平移，不会被丢弃。** 这是迁移中最大的技术风险被消除。

### 5.5 待验证的硬骨头（已由调研修正 — 风险低于预估）

**调研结论：这个代码库已经为新内核预留好了接缝。** 三个 hook 全部声明窄接口：

| 接缝 | 位置 | 内容 |
|---|---|---|
| `ComposerEditorRef` | `use-composer-submit.ts:47-54` | `{ getMarkdown, blur, focus }`——**S0 刚做的** |
| `LazyEditorHandle` | `use-lazy-editor.ts:10-15` | `{ focus, focusAtCoords?, focusAtAnchor?, uploadFile? }`——除 `focus` **全可选** |
| `UploadGate` | `use-upload-gate.ts:54-57` | 只要 `hasActiveUploads()` |

→ **新内核实现这三个接口，Tier 2/3 的提交与 lazy 逻辑可零改动。**

**两个"硬骨头"缺失的后果是降级，不是损坏**（`use-lazy-editor.ts:103-105`）：

```ts
else handle.focus();   // focusAtAnchor / focusAtCoords 缺失时退回默认聚焦
```

即"插入点不落在鼠标位置"，而非功能不可用。
（且可行：LobeHub 自己的 `plugins/upload/utils` 就用 `document.caretRangeFromPoint` 做坐标→range 转换。）

**唯一真正的硬绑定是一处**：`useCoordinatedUploads` 把 `RefObject<ContentEditorRef>` 写死在
签名里（`use-coordinated-uploads.ts:61,93,246`），且要求 `insertUploadPlaceholder` /
`settleUploadPlaceholder` / `insertMarkdownAtEnd` 三个命令式方法，语义刁钻——返回 `boolean`
表示"有没有落地"，未初始化时要能**重试**。

→ **风险形状从"14 处各自可能出错"收敛为"1 处必须做对"。**

### 5.5.1 新发现的不兼容（必须解决）

**LobeHub 的上传 API 不携带 uploadId，会切断我们的身份链。**

```ts
ReactFilePlugin: { handleUpload: (file: File) => Promise<{ url: string }> }
INSERT_FILE_COMMAND: LexicalCommand<{ file: File }>
```

而我们的契约要求（`content-editor.tsx:136-141`、`use-coordinated-uploads.ts:210-218`
两处专门写了警告）：

> 编辑器铸 id → 宿主必须采纳为 `clientUploadId` → 这是"上传活得比 mount 长"时
> 找回占位符节点的**唯一依据**。

**结论：走甲，且已验证可行 —— 但形态比原设想的更好。**

原写"扩展 `FileNode`"其实不是最优解。LobeHub 插件 API 提供了
`kernel.registerNodes(nodes: LexicalNodeConfig[])`，所以正确做法是
**注册我们自己的节点**，把我们现有的语义**原样搬到 Lexical**上：

- `clientUploadId` 身份链
- `uploading` 占位态
- pending 时 markdown 输出为空

差别在于：扩展 `FileNode` 会让我们的核心不变量**依附**于第三方的节点实现（它重构我们就坏）；
自注册节点则是**我们持有定义，LobeHub 只提供宿主**。
上传身份链是这个产品的核心不变量，不该被编码进别人的数据模型里。

**已用 LobeHub 的 `FilePlugin` 源码验证模板**（`es/plugins/file/plugin/index.js`）：

```js
kernel.registerNodes([FileNode]);                       // 1. 注册节点
this.registerDecorator(kernel, type, ...);              // 2. 渲染
onInit(editor) {
  kernel.requireService(IUploadService)?.registerUpload(async (file, from, range) => {
    const fileNode = $createFileNode(file.name);        // 3. 节点由插件自建 → id 由我们铸
    $insertNodes([fileNode]);
    handleUpload(file).then(url => fileNode.setUploaded(url.url));
  });
  registerFileCommand(editor, handleUpload);
  litexmlService.registerXMLWriter/Reader(type, ...)    // 4. 文档序列化
  markdownService.registerMarkdownWriter(type, ...)     // 5. markdown 输出
}
```

**关键**：节点由插件自己创建（`$createFileNode(file.name)`，**只传 name，无 id**），
所以"铸 id → 回传宿主"完全在插件内部完成，**不需要改 LobeHub 任何代码**：

```js
const clientUploadId = createSafeId();
const node = $createAttachmentNode({ ..., clientUploadId, uploading: true });
await onUploadFile(file, clientUploadId);   // 契约保住
```

**markdown writer 是"占位符不进草稿"的落点**：LobeHub 对 pending 输出
`Uploading ${name}...`，我们注册自己的 writer 让它输出**空**，与
`extensions/index.ts:124` 的现有行为逐条一致。

**另一处模型差异**：我们的占位符**不进草稿**（`extensions/index.ts:124` 一行定死
`if (node.attrs?.uploading === true || !src) return "";`），草稿里存的是**独立记录**
`DraftUpload{clientUploadId,status,...}`，靠 id 关联。而 LobeHub 的 `FileNode` **可序列化**
（`exportJSON`）。需决定：pending 态的 FileNode 要不要进草稿。倾向**保持我们的模型**（不进）。

### 5.5.2 S2 验收清单（按方法反查真实依赖面 · 全仓 grep 验证）

| 待实现方法 | 依赖它的调用点（验收必须过） |
|---|---|
| `adoptContent` | `chat-input.tsx:363,391`、`issue-detail.tsx:2954`、`comment-card.tsx:414` — **共 4 处** |
| `flushPendingUpdate` | `chat-input.tsx:352,388,519,534` + `comment-input`/`reply-input`/`comment-card`/`create-issue`/`quick-create` 各 2 处 = **6 文件 14 处**（`feedback` 是唯一没有的） |
| `focusAtCoords` / `focusAtAnchor` | **仅 `comment-input.tsx:66`、`reply-input.tsx:92`**（经 `useLazyEditor`）。`issue-detail.tsx:1958` 那个包的是 `TitleEditor`，自带实现 |
| `insertUploadPlaceholder`/`settleUploadPlaceholder`/`insertMarkdownAtEnd` | 只经 `use-coordinated-uploads.ts:146,150,193,199,345`。接引擎的：chat + comment-input + reply-input + comment-card + create-issue + quick-create；**`feedback` 不接** |
| `pasteAsFileThreshold` | **全仓唯一 `chat-input.tsx:698`**，B 层 6 个都不用 → 迁移 B 层时可完全绕开 |

**两个由此得出的结论**：
- `focusAtCoords`/`focusAtAnchor` 影响面很窄（2 个调用点），且缺失时降级而非损坏 → **可以留到最后做，甚至做到 Tier 3 时再决定**。
- `pasteAsFileThreshold` 是纯 A 层需求 → **B 层迁移可以先绕开它**，进一步缩小 S2 的首个可用版本。

### 5.5.3 调研范围边界（避免过度读取）

上面的矩阵**只测绘了 B 层那 6 个调用点**。A 层（`chat-input.tsx`）和 C 层 7 个长文档/配置
编辑器**没有做 props 与方法矩阵**。按 S4/S5 推进前，A 层与 C 层需各自单独测绘一轮——
尤其 C 层的 `instructions-editor`（Agent 配置源）与 automation 配置。

### 5.6 分阶段执行

- [x] **S0 依赖倒置**：`useComposerSubmit` 的 `editorRef` 从 `ContentEditorRef` 收窄为 `ComposerEditorRef`（详见 §6）。**7 个调用点零改动**，1663 测试全绿。
- [x] **S2a 上传地基**：节点 / 插件 / 操作层 / React 绑定 / 编辑器组件，全部自带测试（§5.8）。**MUL-5181 身份链已在新内核上跑通**。
- [~] **S1 契约固化**：部分达成——现有测试已可用作预言机（耦合极浅：`content-editor.test.tsx` 仅 5 处 TipTap 引用）。改为「新实现跑旧测试」而非另写契约套件。
- [~] **S2b 适配层**：已完成 `flushPendingUpdate`（自持防抖）、`adoptContent`、`insertMarkdownAtEnd`、
  `insertUploadPlaceholder`/`settleUploadPlaceholder`/`failAttachment`、`onSubmit`（⌘+Enter）、
  `flushPendingOnUnmount`、**`value` 受控同步（含回声抑制）**。
  **未完成**：`focusAtCoords`/`focusAtAnchor`、mention、slash、气泡菜单（`ReactToolbarPlugin`）、
  `pasteAsFileThreshold`、两参数 `onUpdate(md, baseMarkdown)`（`issue-detail` 需要）、
  `attachments` 播种、`currentIssueId`/`onReady`、以及**接 `useCoordinatedUploads`**（唯一硬绑定）。

  **`value` 受控同步的实现要点**：`lastSyncedValueRef` 同时充当回声过滤器与"已见过"标记，
  且在写入文档**之前**推进——因为同步 effect 只在 `value` **变化**时触发，标记留在后面会让
  每个后续渲染重复检查同一个陈旧值。debounce 触发时**先记录再 emit**，这样受控宿主把我们的
  `onUpdate` 原样回传成下一个 `value` 时，effect 能识别为自己的回声而非外部变更。
- [ ] **S3 切到 A 层**（`chat-input.tsx`）—— 用户最想先看到动效处，Electron 验证。**前置**：mention + slash + `value` 受控 + `pasteAsFileThreshold`。
- [~] **S4 铺开 B 层 6 处**：**Tier 1 `feedback.tsx` 已完成并验证**。剩 Tier 2（`create-issue`、`quick-create`，需协同上传引擎）与 Tier 3（comment 三件套，另需 lazy + slash + `adoptContent`）。
- [ ] **S5 铺开 C 层 7 处**——**逐个验证序列化**，`instructions-editor`（Agent 配置源）与 automation 配置**最后做**。**C 层尚未测绘**。
- [ ] **S6 全量回归 + Electron 验证 + 删除旧 TipTap 实现**。

> `useCoordinatedUploads` 的接口适配（`RefObject<ContentEditorRef>` 写死在
> `use-coordinated-uploads.ts:61,93,246`）是 S2b 的核心，也是唯一一处真正的硬绑定。

> **S3 不能先于 S2。** `chat-input.tsx` 的 `ContentEditor` 同时还喂着
> `mentionMode="context"` / `enableSlashCommands` / `showBubbleMenu` /
> `pasteAsFileThreshold` / `attachments`。只换外壳会**静默丢掉**提及、斜杠命令、
> 格式菜单、粘贴转文件、附件节点。必须先在 LobeHub 上补齐这些能力，再切。

### 5.7 已完成的 S0 + composer 骨架

**`LobeComposer`（`packages/views/chat/lobe/lobe-composer.tsx`）** 已重写为符合仓库
发送契约的形态。过程中确认/修正了四件事：

| 事实 | 影响 |
|---|---|
| `ChatInput` 是**纯布局容器**（`re-resizable` + header/footer 槽位），不依赖 Lexical | 分层干净，将来可只换 body 里的编辑器 |
| `SendButton` 有 `generating` 态，**会变形为停止按钮**并调 `onStop` | 用户要的动效之一，已接 |
| 库的 `onKeyDown` **嵌套在 `if (editor && onPressEnter)` 分支内** —— 只传 `onKeyDown` 会静默失效 | 必须走 `onPressEnter`。已避开 |
| `isShortcutAllowedForAction` 把 `send` **限制为仅 `Enter` / `primary+Enter`** | 所以 Enter 系钩子覆盖全部合法绑定，`onPressEnter` 是正确选择 |

**丢弃了 `shouldReplayNativeEnter`**：它补偿的是 TipTap 遮蔽自家 Shift+Enter 的行为，
Lexical 原生软换行已正确，照搬是死代码。而 `shouldHandleSubmitShortcut` 是纯函数、与框架无关，
**直接复用**——中文 IME 的 Safari 兜底（`keyCode === 229`）因此得以保留。

**修正了我自己写下的一个契约违规**：初版 `handleSend` 是 `cleanDocument()` 后再
`await onSend()`，即**乐观清除**。这与仓库既定契约冲突（CLAUDE.md：chat 发送用
pending-message 模式；`useComposerSubmit` 文档："a rejected send keeps the draft for retry"）。
已改为**由宿主清除**：composer 只发 `onSubmit()` 信号，宿主在 `onAccepted` 里调
`clearContent()`。

### 5.8 S2 地基（已完成并验证）

新增 `packages/views/editor/lobe/`，**全部为新增文件，未触碰任何现有调用点**：

| 文件 | 测试 | 职责 |
|---|---|---|
| `attachment-node.ts` | 7 | Lexical `DecoratorNode`，承载 `clientUploadId` 身份链 |
| `attachment-plugin.ts` | 6 | 内核插件；**"pending/error 不产出 markdown"** 不变量 |
| `attachment-ops.ts` | 10 | 幂等占位符 / settle / fail / remove / 门控 / 列表 |
| `react-attachment-plugin.tsx` | — | React 绑定 + 卡片视觉（三态） |
| `upload-result.ts` | — | `UploadResult` → 节点存储形态（选定进 markdown 的 URL） |
| `lobe-content-editor.tsx` | 8 | 组件：内核+插件+节点+decorator+markdown 全链路 |

**设计要点**：

1. **`LobeContentEditor` 刻意不声明 `ContentEditorRef`**。它 15 个成员里多数属于尚未迁移的
   协同上传与 mention/slash 管线。声明了却用空实现填坑是最坏结果——调用方全部通过类型检查，
   而依赖未实现成员的那些**在运行时静默失效**。所以只声明真正实现的子集。

2. **`useLexicalComposerContext` 的上下文归属**：`EditorProvider` 提供的是 kernel，
   Lexical composer 由 `Editor` 自己创建。所以插件必须走 `Editor` 的 `plugins` prop
   （或作为其 children），作为兄弟节点挂载会抛 `cannot find a LexicalComposerContext`。

3. **`useLexicalNodeSelection` 未公开导出**（exports map 无通配），节点选中改用 Lexical
   自己的 `$createNodeSelection` + `$setSelection`——不引额外依赖。

4. **类型反推**：`IEditorKernel` / `IEditorPluginConstructor` / `IDecorator` /
   `IMarkdownWriterContext` 全部未导出。从公开的 `IEditor.registerPlugin` 与
   `IMarkdownShortCutService` 反推，避免结构复刻走样。
   但 `Parameters<泛型方法>` 会把配置类型塌缩成 `unknown`，故 `AttachmentPluginConstructor`
   需手写签名才能让 `registerPlugin` 推出 `AttachmentPluginOptions`。

5. **测试暴露的真实 API 缺口**：原先只有 `settleAttachment`，调用方要自己拿节点调
   `setError`——而 Lexical 要求变更必须在 `update()` 内，外部调用会运行时抛错。补了对称的
   `failAttachment`。

### 5.9 已迁移的第一个真实调用点（S4 Tier 1）

**`packages/views/modals/feedback.tsx` 已跑在 Lexical 内核上。**

选它做试点的理由（来自调研）：6 个 B 层调用点里**唯一不接协同上传引擎**的——其 ref 用法只有
`getMarkdown` + `uploadFile`。最小可验证路径。

过程中触及的两处共享代码：

| 改动 | 说明 |
|---|---|
| `use-upload-gate.ts` | `editorRef` 从 `ContentEditorRef` 收窄为 `UploadGateEditor`（只要 `hasActiveUploads()`）。**第二次用 S0 的同一手法**，调用点零改动 |
| `editor/lobe/index.ts` + package.json | 新增 `./editor/lobe` 子路径导出 |

**修掉的两个真 bug：**

1. **`uploadFile` 对 handler 返回值直接调 `.then()`** —— TipTap 版用的是 `await handler(...)`。
   `await undefined` 合法，`undefined.then()` 抛错。任何返回 void 的宿主（含测试 mock）都会崩。
   改为 `await`。
2. **模块图污染** —— 我一度把 `./lobe` 从 `editor/index.ts` 重新导出，导致**每一个 `../editor`
   消费者**都被拖进 `@lobehub/ui` 根 → `SortableList` → `@dnd-kit/core`，而
   `issues-page.test.tsx` 的 dnd-kit mock 没有 `defaultDropAnimationSideEffects`，测试文件直接
   加载失败。已撤回 re-export，调用点改走子路径。

   > **教训**：我当初 re-export 是为了让 `vi.mock("../editor")` 能拦截——**让测试便利性驱动架构
   > 决策是搞反了**。模块图隔离比单一导入路径值钱，且这正是 Phase 2 那次 deep-import 修复奏效的
   > 同一个原因。

### 5.9.1 C 层测绘结果（7 个文件，已查证）

| # | 文件:行 | 内核语义 | ref | 难度 |
|---|---|---|---|---|
| 1 | `issues/components/issue-detail.tsx:2892` | **受控 + 上传 + 冲突合并** | 3 个方法 | **最险** |
| 2 | `projects/components/project-detail.tsx:457` | 受控自动保存 | 有但**未使用** | 中 |
| 3 | `modals/create-project.tsx:420` | 非受控 draft | `getMarkdown` | 低 |
| 4 | `teams/components/team-detail-page.tsx:1366` | 受控 + 显式保存 | **无** | 低 |
| 5 | `agents/components/instructions-editor.tsx:104` | 非受控 | `focus` | **疑似死代码** |
| 6 | `automations/components/automation-edit-dialog.tsx:544` | 非受控 | **无** | 低 |
| 7 | `automations/components/automation-settings-sections.tsx:77` | 受控（`canWrite` 分支） | 可选透传 | 中 |

**三条改变计划的结论：**

1. **C 层完全不用 mention / slash / `onSubmit` / `onBlur` / `onReady` / `onUploadingChange`。**
   → C 层比 B/A 层简单得多，不需要前置那五项能力。

2. **但 `value` 受控被 4/7 使用**（#1 #2 #4 #7）——这是我一直推迟的硬骨头（含 Guard 0 那套
   skip 守卫），**C 层绕不过去**，必须在 S5 之前完成。

3. **`issue-detail.tsx` 全仓唯一用两参数 `onUpdate(md, baseMarkdown)`** ——
   第二参作为 `description_base` 发给服务端做合并。新内核的 `onUpdate` 只给一个参数，
   **需要专门补上第二个**，否则描述编辑的冲突合并会静默失效。
   它也是 **C 层唯一用 `attachments` 和 `currentIssueId` 的调用点**。

**`disableMentions`**（#4 #5）在无 mention 的新内核上是天然满足的。

### 5.9.2 【最高优先级】`onUpdate` 第二参缺口 —— 数据损坏级风险

**这是整个迁移里最危险的一处，且我的实现目前是错的（缺口真实存在）。**

`issue-detail.tsx:2895` 用两参数 `onUpdate(md, baseMarkdown)`。第二参**不是冗余设计**，
它是服务端区分「用户删除了渠道媒体」与「并发写入」的**唯一依据**。

**服务端**（`server/internal/handler/issue.go:3617-3661`）：

```
mergeIssueChannelMediaDescription(current, incoming, base, attachments)
  base 已知  且 用户删了链接   → 尊重删除 ✓
  base 未知  且 无链接         → 追加 Block()   ← 复活用户已删的媒体 ✗
  有链接     且 无 marker      → 补回 Marker()
```

两处调用：`:3819` 冲突探测（→ 409 `err_field_conflict`）、`:3825` 真正写入。

**客户端的硬约束**（`content-editor.tsx:90-96` docstring + `:414-418`）：

- base 是「**最后一次真正被采纳**的受控值」——docstring 明令
  **禁止用最新 prop 顶替**（脏编辑器的守卫会故意跳过更新的服务端内容）；
- `documentBaseRef` 必须保存**原始受控字符串，而非内核序列化产物**——因为序列化会丢
  HTML 注释形式的 `<!-- orvilo:channel-media:<uuid> -->` marker，而服务端需要在 base 里看到它。

**→ 新内核若把序列化后的可见 markdown 当 base，服务端会把用户已删除的渠道媒体重新追加回来。**

**S5 的落点**：需要独立的 `documentBaseRef`（语义与我的 `lastSyncedValueRef` **不同**——
后者在 emit 时更新用于回声抑制，前者只在**采纳外部 value 时**更新）：

| 时机 | 动作 |
|---|---|
| 挂载 | 用初始 `value` / `defaultValue` 初始化 |
| 应用外部 `value` 时 | 更新为**原始字符串** |
| 脏态守卫跳过新 value 时 | **绝不更新** |
| 编辑器自身编辑 emit | **不更新** |

现状：`lobe-content-editor.tsx` 声明的是单参 `onUpdate?: (markdown: string) => void`。

### 5.9.3 另外三处形状敏感的序列化边界（无断言，坏了不报错）

| 位置 | 服务端拼接 | 风险 |
|---|---|---|
| `service/automation.go:1741-1747` | `"\n\n---\n*Automation run triggered at …"` + webhook JSON | 用户文本以 `---` 结尾或留下**未闭合代码围栏**会吞掉追加块 |
| `handler/team_briefing.go:186-191` | `"\n\n## Team Instructions (" + name + ")\n\n"` | 用户内容以 `##` 开头会与系统标题层级混淆 |
| `channelmedia/markdown.go:10` | `<!-- orvilo:channel-media:<uuid> -->` 正则 | 见 5.9.2 |

**产品代码真正依赖的 markdown 形状只有三种正则**：channel-media marker、mention 语法
（`util/mention.go:14`）、附件 durable path（`util/attachment_url.go`）。
**表格语法 / 标题层级 / 列表符号：未查到依赖**（= 未找到证据，非确认不存在）。

**附件绑定同样依赖 durable path**：`issue-detail.tsx:2911-2915` 的
`contentReferencesAttachment` 匹配 `/api/attachments/<id>/download`，**不是 `attachment.url`**。
→ 我的 `toSettleResult` 取 `result.markdownLink`（由 `pickMarkdownLink` 优先选 `markdown_url`）
方向正确，但**迁移后必须实测这条 URL 真的出现在 `getMarkdown()` 输出里**。

### 5.9.4 `instructions-editor.tsx` 是死代码 —— 证据确凿，直接删

四条独立检查全部指向同一结论：全仓库标识符扫描 3 处（全在自己文件内）、模块路径 import
**0** 处、动态 `import()`/`lazy()` **0** 处、大小写不敏感同样 3 处。
排除了「被 import 但路径不可达」这一可能——**根本没有 import 语句**。

旁证：`agents/components/index.ts` 不含它；`create-agent-dialog.test.tsx:509` 有用例名
"does not render or submit description, instructions, or skills"；真正的 agent instructions
用 `<Textarea>`（`tabs/instructions-tab.tsx:9`）。

→ **S5 直接删除，不构成工作量。**（它长得像活代码——完整 props 接口 + 详尽 JSDoc——但零引用。
「死代码」要靠引用计数证明，不能靠形态判断。）

### 5.9.5 测试覆盖的真实状况（重要警告）

**这 7 个 host 的测试全部把 `ContentEditor` mock 掉，没有任何一条跨过真实 markdown 序列化边界。**

- #4 `team-detail-page` / #5 `instructions-editor`：**完全没有测试文件**
- #2 `project-detail`：`ContentEditor: () => null`，描述编辑完全未测
- #3 `create-project`：mock 成 textarea 且**未定义 `getMarkdown`**
- #1 `issue-detail`：用例齐全（含 unmount flush、keyed 重挂载、冲突草稿、采用服务端版本）
  但**全走 mock，不覆盖 channel-media 往返**
- e2e：**无**覆盖这 7 个编辑器的 spec

> **→ 现有测试全绿不能证明 C 层迁移正确。**必须在迁移后为 #1/#4/#7 各补一条**跨真实序列化
> 边界**的 round-trip 测试，否则上面那些形状敏感的拼接点坏了 CI 抓不到。

### 5.9.6 C 层只需 4 个 ref 方法

`uploadFile`、`adoptContent`（#1）· `getMarkdown`（#3、#7）· `focus`（#5）。
**#4 / #6 完全不用 ref；#2 声明了 ref 但从未使用（死 ref，可顺手清）。**
C 层未出现：`clearContent` / `focusAtCoords` / `focusAtAnchor` / `blur` / `hasActiveUploads` /
`insertMarkdownAtEnd` / 占位符三方法 / `flushPendingUpdate`。

**C 层也确认没有 `ContentEditor` + `useLazyEditor` 组合**——`issue-detail.tsx:1958` 的 lazy
只作用于 `TitleEditor`，且 `:1935-1940` 注释明确说明描述编辑器**故意保持 eager**
（长文档在 react-markdown ↔ ProseMirror 间切换会累积高度差导致滚动跳变）。
`focusAtCoords`/`focusAtAnchor` 的真实需求面仍只有 B 层的 `comment-input` + `reply-input`。

### 5.9.7 C 层迁移顺序

1. **第一梯队**：#6 `automation-edit-dialog`（非受控、无 ref）→ #5 **删除** →
   #4 `team-detail-page`（无 ref，但要求高：`disableMentions` 语义 + 150ms 后落库文本一致，
   且内容进 leader prompt）
2. **第二梯队**：#3 `create-project` → #2 `project-detail`（顺手清死 ref）→
   #7 `automation-settings-sections`（受控 + `flushPendingOnUnmount` + `canWrite` 翻转会
   **卸载重挂** + create 页把 `debounceMs` 覆盖为 0）
3. **第三梯队（最后）**：#1 `issue-detail` —— 四条约束须逐条守住：
   ① `documentBaseRef` 用原始受控值；② `adoptContent` 必须「应用但**不触发** `onUpdate`」；
   ③ `flushPendingOnUnmount` 在 1500ms debounce 下真刷出最后一次粘贴；
   ④ `key={id}` 换 issue 不携带旧文档

### 5.10 验证状态

```
pnpm exec tsc --noEmit  → 0 error（整个 packages/views）

vitest run editor/ modals/ issues/ chat/ projects/ teams/ agents/ automations/
  → 295 files / 3463 tests passed
```

分项：

| 套件 | 结果 |
|---|---|
| `editor/lobe/`（新内核地基） | 34 passed |
| `modals/feedback.test.tsx`（已迁移的试点） | 8 passed |
| 其余全部（未迁移的调用点） | 通过，无回归 |

**注意**：`feedback.test.tsx` 把 `../editor/lobe` 整个 mock 掉（换成 textarea），所以它验证的是
**modal 的接线**（props 传递、上传门控、提交路径）；真实 Lexical 组件的行为由
`lobe-content-editor.test.tsx` 覆盖。**真实 DOM 下的表现仍未经 Electron 验证**（S6）。

---

## 六、Phase 4 与验证

- **C13 `prefers-reduced-motion`**：**已存在**，无需新增。`packages/ui/styles/base.css:298-308` 已覆盖全部 8 个 `animate-*` 类。（我在初版方案里断言"一个都没做"是错的 —— 基于 grep 部分结果下的结论，已纠正。）
- **清理**：临时验证产物（`spike` 路由、`proxy.ts` 公开路由改动、`spike-panel.tsx`）**全部已删/已回退**，`git status` 确认干净。
- **回归**：`packages/views` 全量 5158 passed，优于基线。

### Phase 3 之前的改动（已完成，S1–S6 之前）

```
M  packages/views/editor/use-composer-submit.ts   (收窄 editorRef 到 ComposerEditorRef)
?? packages/views/chat/lobe/lobe-composer.tsx     (LobeHub composer + 命令式 handle)
```

**`useComposerSubmit` 的依赖倒置**（已完成，是本阶段第一个正确决策）：
该 hook 此前声明 `editorRef: RefObject<ContentEditorRef>`，但它**只用了三个方法**——
`getMarkdown()`(L151) / `blur()`(L136) / `focus()`(L145)。
现已收窄为 `ComposerEditorRef`（声明在消费者处）。`ContentEditorRef` 结构上仍满足它，
**7 个调用点零改动**，却让该 hook 不再绑定具体编辑器实现。

### 改动清单

```
M  packages/views/chat/components/chat-message-list.tsx   (44+ / 33-)
M  packages/views/package.json                            (新增 6 个依赖)
M  packages/views/vitest.config.ts                        (inline + testTimeout)
M  pnpm-workspace.yaml                                    (onlyBuiltDependencies)
M  pnpm-lock.yaml
?? packages/views/chat/lobe/                              (5 个源文件 + 3 个测试)
```

### 已知风险（未解）

| 风险 | 实测值 | 说明 |
|------|--------|------|
| bundle 体积 | 主 bundle **24.6 MB**（未压缩） | antd 6 + Lexical + Emotion + mdx。生产构建会 minify，但增量显著，Electron 冷启动需实测 |
| 测试环境变慢 | 单套件从毫秒级到秒级 | antd 在 jsdom 里逐次解析 CSS-in-JS 样式树。已把 `testTimeout` 提到 20s；**真实浏览器不受影响** |
| 双样式运行时 | antd-style(Emotion) + Tailwind | 目前靠 CSS `@layer` 隔离良好，但这是长期并存的维护面 |
| `@lobehub/ui/es/*` deep import | — | 依赖包内部结构；若 LobeHub 重构需跟随 |

---

## 七、下一轮建议的起点

1. **先决定 Phase 3 的路线（A/B/C）** —— 这是剩余工作的全部不确定性所在。
2. 若要 A 或 C，建议独立开分支，并把 `ContentEditorRef` 的接口契约先固化成测试，再动内核。
3. 顺带修一个既有问题：`packages/ui/styles/base.css` 的 `border-beam` 里有硬编码色值 `#ffbe7b` / `#ff777f`（`base.css:239-243` 与 `209-212`），违反仓库"禁止硬编码颜色"规则。
