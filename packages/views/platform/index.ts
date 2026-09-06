export { useImmersiveMode } from "./use-immersive-mode";
export { useDesktopUnreadBadge } from "./use-desktop-unread-badge";
export { DragStrip } from "./drag-strip";
export { openExternal } from "./open-external";
export {
  isDesktopShell,
  pickDirectory,
  cloneProjectRepository,
  confirmProjectRepository,
  pickDirectories,
  validateLocalDirectory,
  type PickDirectoryResult,
  type PickDirectoriesResult,
  type ValidateLocalDirectoryResult,
} from "./local-directory";
export {
  useLocalDaemonStatus,
  type LocalDaemonStatus,
} from "./use-local-daemon-status";
export {
  ScrollRestorationProvider,
  useScrollRestorationAdapter,
  useRestoredScrollEntry,
  useRestoredScrollOffset,
  useRestoredScrollRef,
  useRestoredViewState,
  useViewStateWriter,
  type ExternalScrollSource,
  type ScrollRestorationAdapter,
  type ScrollRestorationEntry,
} from "./scroll-restoration";
