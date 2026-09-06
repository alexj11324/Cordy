import { app, ipcMain, dialog, BrowserWindow } from "electron";
import { access, stat } from "fs/promises";
import { constants as fsConstants } from "fs";
import { basename, dirname, isAbsolute, join } from "path";
import { preferredAppLocaleFromLanguages } from "./os-locale";
import { cleanRemote, inspectRepository, checkRepositoryAccess, cloneRepository, repositoryIdentity } from "./local-repository";

const repositoryDialogCopy = {
  en: ["Choose a new folder for this repository", "Clone here", "Check project folder", "Cancel", "Bind this folder", "This folder does not match the project's remote repository.", "It may be a fork or a repository without a remote. Bind it only if this is the code you want tasks to use. Existing files will be preserved."],
  "zh-Hans": ["选择用于存放仓库的新文件夹", "下载到这里", "确认项目文件夹", "取消", "绑定此文件夹", "此文件夹与项目的远程仓库不匹配。", "它可能是 fork 或尚未配置远程地址的仓库。请确认这是任务应使用的代码，再进行绑定。已有文件将保留。"],
  ja: ["リポジトリの保存先を選択", "ここにダウンロード", "プロジェクトフォルダーを確認", "キャンセル", "このフォルダーを接続", "このフォルダーはプロジェクトのリモートと一致しません。", "フォークまたはリモート未設定の可能性があります。タスクで使用するコードか確認してください。既存のファイルは保持されます。"],
  ko: ["저장소를 저장할 새 폴더 선택", "여기에 다운로드", "프로젝트 폴더 확인", "취소", "이 폴더 연결", "이 폴더가 프로젝트의 원격 저장소와 일치하지 않습니다.", "포크이거나 원격 주소가 없을 수 있습니다. 작업에 사용할 코드인지 확인하세요. 기존 파일은 보존됩니다."],
} as const;
function repositoryCopy() {
  return repositoryDialogCopy[preferredAppLocaleFromLanguages(app.getPreferredSystemLanguages?.() ?? ["en"])];
}

export interface PickDirectoryResult {
  ok: boolean;
  path?: string;
  basename?: string;
  /** Set when ok=false. "cancelled" = user dismissed; otherwise an error blurb. */
  reason?: "cancelled" | "no_window" | "error";
  error?: string;
}

export interface ValidateLocalDirectoryResult {
  ok: boolean;
  /** When ok=false, identifies which check failed so the renderer can render a
   *  specific message without parsing free-form text. */
  reason?:
    | "not_absolute"
    | "not_found"
    | "not_a_directory"
    | "not_readable"
    | "not_writable"
    | "error";
  error?: string;
  /**
   * Whether the directory sits inside a git working tree. Only set when ok=true.
   *
   * Worktree execution mode requires a git repo, and only the desktop app can
   * see the filesystem — the server cannot. Reporting it here lets the picker
   * disable that mode with a reason at selection time, instead of letting the
   * user save a resource whose very first task fails.
   */
  is_git_repo?: boolean;
  has_commits?: boolean;
  remotes?: Array<{ name: string; url: string }>;
}

type PickDirectoriesResult = {
  ok: boolean;
  folders?: Array<{ path: string; basename: string }>;
  reason?: "cancelled" | "no_window" | "error";
  error?: string;
};

export async function validateLocalDirectory(
  path: string,
): Promise<ValidateLocalDirectoryResult> {
  if (!path || !isAbsolute(path)) {
    return { ok: false, reason: "not_absolute" };
  }
  try {
    const st = await stat(path);
    if (!st.isDirectory()) return { ok: false, reason: "not_a_directory" };
  } catch (err) {
    const code = (err as NodeJS.ErrnoException).code;
    if (code === "ENOENT") return { ok: false, reason: "not_found" };
    return { ok: false, reason: "error", error: errorMessage(err) };
  }
  try {
    await access(path, fsConstants.R_OK);
  } catch {
    return { ok: false, reason: "not_readable" };
  }
  try {
    await access(path, fsConstants.W_OK);
  } catch {
    return { ok: false, reason: "not_writable" };
  }
  const isGitRepo = await isInsideGitWorkTree(path);
  return { ok: true, is_git_repo: isGitRepo, ...(isGitRepo ? await inspectRepository(path) : {}) };
}

/**
 * Walks up from `path` looking for a `.git` entry, mirroring how git itself
 * resolves a working tree — so a subdirectory of a repo reports true, matching
 * what the daemon does with `rev-parse --show-toplevel` at task time.
 *
 * `.git` is accepted as either a directory (ordinary clone) or a file (a linked
 * worktree, where it holds a gitdir pointer). Any error means "can't tell",
 * which is reported as not-a-repo: this only drives a UI hint, and the daemon
 * re-checks authoritatively before running anything.
 */
async function isInsideGitWorkTree(path: string): Promise<boolean> {
  let current = path;
  for (;;) {
    try {
      await stat(join(current, ".git"));
      return true;
    } catch {
      // Not here — keep walking up.
    }
    const parent = dirname(current);
    if (parent === current) return false;
    current = parent;
  }
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/**
 * Registers the directory picker.
 *
 * `onDirectoryChosen` is invoked with each path the user actually selected in
 * the OS dialog. The local Guest runner uses it to build the set of
 * directories it is allowed to run in — a path the renderer merely names is
 * not consent, and the dialog is the one moment consent is given.
 */
export function setupLocalDirectory(
  windowGetter: () => BrowserWindow | null,
  onDirectoryChosen?: (path: string) => void | Promise<unknown>,
): void {
  ipcMain.handle("local-directory:clone", async (event, remote: string) => {
    try {
    const win = BrowserWindow.fromWebContents(event.sender) ?? windowGetter();
    if (!win) return { ok: false, reason: "no_window" };
    const url = typeof remote === "string" ? cleanRemote(remote) : null;
    if (!url) return { ok: false, reason: "invalid_url" };
    const accessResult = await checkRepositoryAccess(url);
    if (!accessResult.ok) return accessResult;
    const result = await dialog.showSaveDialog(win, {
      title: repositoryCopy()[0],
      buttonLabel: repositoryCopy()[1],
      defaultPath: basename(new URL(url).pathname).replace(/\.git$/, ""),
      properties: ["createDirectory", "showOverwriteConfirmation"],
    });
    if (result.canceled || !result.filePath) return { ok: false, reason: "cancelled" };
    const cloned = await cloneRepository(url, result.filePath);
    if (!cloned.ok) return cloned;
    // Clone success is independent of the optional Guest consent registry.
    // The project binding uses this returned path directly.
    return { ok: true, path: result.filePath, basename: basename(result.filePath) };
    } catch { return { ok: false, reason: "error" }; }
  });
  ipcMain.handle("local-directory:confirm-repository", async (event, path: string, expected: string[]) => {
    const win = BrowserWindow.fromWebContents(event.sender) ?? windowGetter();
    if (!win || !isAbsolute(path) || !Array.isArray(expected)) return false;
    const repository = await inspectRepository(path);
    const identities = expected.filter(value => typeof value === "string").map(repositoryIdentity).filter(Boolean);
    if (!identities.length || repository.remotes.some(remote => identities.includes(repositoryIdentity(remote.url)))) return true;
    const answer = await dialog.showMessageBox(win, {
      type: "warning", title: repositoryCopy()[2], buttons: [repositoryCopy()[3], repositoryCopy()[4]],
      defaultId: 0, cancelId: 0,
      message: repositoryCopy()[5],
      detail: repositoryCopy()[6],
    });
    return answer.response === 1;
  });
  ipcMain.handle(
    "local-directory:pick",
    async (event, defaultPath?: string): Promise<PickDirectoryResult> => {
      const win = BrowserWindow.fromWebContents(event.sender) ?? windowGetter();
      if (!win) return { ok: false, reason: "no_window" };
      try {
        const result = await dialog.showOpenDialog(win, {
          // Multiple-selection is intentionally disabled — a project_resource
          // points at a single directory, and the create flow expects one
          // path per click. Multi-add would have to be a separate UX.
          properties: ["openDirectory", "createDirectory"],
          ...(defaultPath ? { defaultPath } : {}),
        });
        if (result.canceled || result.filePaths.length === 0) {
          return { ok: false, reason: "cancelled" };
        }
        const picked = result.filePaths[0];
        if (!picked) return { ok: false, reason: "cancelled" };
        await onDirectoryChosen?.(picked);
        return { ok: true, path: picked, basename: basename(picked) };
      } catch (err) {
        return { ok: false, reason: "error", error: errorMessage(err) };
      }
    },
  );

  ipcMain.handle(
    "local-directory:pick-many",
    async (event, defaultPath?: string): Promise<PickDirectoriesResult> => {
      const win = BrowserWindow.fromWebContents(event.sender) ?? windowGetter();
      if (!win) return { ok: false, reason: "no_window" };
      try {
        const result = await dialog.showOpenDialog(win, {
          properties: ["openDirectory", "createDirectory", "multiSelections"],
          ...(defaultPath ? { defaultPath } : {}),
        });
        if (result.canceled || result.filePaths.length === 0) {
          return { ok: false, reason: "cancelled" };
        }
        const folders: Array<{ path: string; basename: string }> = [];
        for (const path of result.filePaths) {
          // The native selection, never renderer-provided paths, grants consent.
          await onDirectoryChosen?.(path);
          folders.push({ path, basename: basename(path) });
        }
        return { ok: true, folders };
      } catch (err) {
        return { ok: false, reason: "error", error: errorMessage(err) };
      }
    },
  );

  ipcMain.handle(
    "local-directory:validate",
    (_event, path: string): Promise<ValidateLocalDirectoryResult> =>
      validateLocalDirectory(path),
  );
}
