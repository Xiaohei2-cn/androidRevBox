/**
 * 远端路径的浏览历史（纯函数，不依赖 React，好单测）。
 *
 * 为什么自己做而不是用 `history.back()`：这是**设备上的路径**，与应用自己的
 * 路由无关；浏览器式的前进/后退语义要按"用户在这台机上走过的目录"来记。
 */

export interface PathHistory {
  /** 走过的路径，从旧到新 */
  entries: string[];
  /** 当前所在位置在 entries 里的下标 */
  index: number;
}

export const initialHistory = (path: string): PathHistory => ({ entries: [path], index: 0 });

export const currentPath = (h: PathHistory): string => h.entries[h.index] ?? "/";

/**
 * 去一个目录：同路径不产生历史；往后走会**丢掉**前进分支
 * （与浏览器一致——去了新地方，原来的"前进"就没意义了）。
 */
export function goTo(h: PathHistory, path: string): PathHistory {
  const target = normalizeRemotePath(path);
  if (!target || target === currentPath(h)) return h;
  const entries = [...h.entries.slice(0, h.index + 1), target];
  return { entries, index: entries.length - 1 };
}

export const canGoBack = (h: PathHistory): boolean => h.index > 0;
export const canGoForward = (h: PathHistory): boolean => h.index < h.entries.length - 1;

export function goBack(h: PathHistory): PathHistory {
  return canGoBack(h) ? { ...h, index: h.index - 1 } : h;
}

export function goForward(h: PathHistory): PathHistory {
  return canGoForward(h) ? { ...h, index: h.index + 1 } : h;
}

/**
 * 上一级（不是历史后退）：`/sdcard/Download` -> `/sdcard`，`/sdcard` -> `/`，`/` -> null。
 * 返回 null 表示已经在根，界面据此禁用按钮，而不是拼出一个 `/..` 之类的假路径。
 */
export function parentOf(path: string): string | null {
  const segments = normalizeRemotePath(path)
    .split("/")
    .filter((segment) => segment.length > 0);
  if (segments.length === 0) return null;
  segments.pop();
  return segments.length === 0 ? "/" : `/${segments.join("/")}`;
}

/**
 * 归一化用户手输的路径：去掉重复斜杠与尾部斜杠（根除外），原地展开 `..`。
 * 不做存在性判断——那是设备侧的事，这里只保证"看起来像一条路径"。
 */
export function normalizeRemotePath(path: string): string {
  const trimmed = path.trim();
  if (!trimmed.startsWith("/")) return trimmed ? `/${trimmed}` : "";
  const out: string[] = [];
  for (const segment of trimmed.split("/")) {
    if (segment === "" || segment === ".") continue;
    if (segment === "..") {
      out.pop();
      continue;
    }
    out.push(segment);
  }
  return `/${out.join("/")}`;
}
