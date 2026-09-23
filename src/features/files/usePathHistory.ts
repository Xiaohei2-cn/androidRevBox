import { useCallback, useEffect, useMemo, useState } from "react";
import {
  canGoBack,
  canGoForward,
  currentPath,
  goBack,
  goForward,
  goTo,
  initialHistory,
  parentOf,
  type PathHistory,
} from "./pathHistory";

/**
 * 浏览历史状态。前进/后退/上一级都只是在这份纯函数状态上做跳转，
 * 规则集中在 `pathHistory.ts` 里并被单测钉住，这里只做 React 接线。
 */
export function usePathHistory(start: string, resetKey?: string | null) {
  const [history, setHistory] = useState<PathHistory>(() => initialHistory(start));

  // 换设备时清空历史：不然会停在另一台机上才有的路径（比如 A 机走到
  // /data/local/tmp/frida，切到没这个文件的 B 机就只能先报错再手动改地址栏）。
  const anchor = resetKey ?? null;
  useEffect(() => {
    setHistory(initialHistory(start));
    // start 只在挂载时有意义，故意不进依赖表
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [anchor]);

  const go = useCallback((path: string) => setHistory((h) => goTo(h, path)), []);
  const back = useCallback(() => setHistory((h) => goBack(h)), []);
  const forward = useCallback(() => setHistory((h) => goForward(h)), []);

  const path = currentPath(history);
  const up = useMemo(() => parentOf(path), [path]);
  const goUp = useCallback(
    () => setHistory((h) => (parentOf(currentPath(h)) ? goTo(h, parentOf(currentPath(h)) as string) : h)),
    [],
  );

  return {
    path,
    go,
    back,
    forward,
    up,
    goUp,
    canBack: canGoBack(history),
    canForward: canGoForward(history),
    /** 历史里已有的条目数，用于界面上"走过几个地方"的极简提示 */
    depth: history.entries.length,
  };
}

export type PathNav = ReturnType<typeof usePathHistory>;
