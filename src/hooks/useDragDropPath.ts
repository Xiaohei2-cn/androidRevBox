import { useEffect } from "react";

/**
 * 访达拖入文件 → 拿宿主绝对路径回调（Tauri v2 webview 级 onDragDropEvent）。
 *
 * 为什么不用 HTML5 drop 的 dataTransfer：webview 沙箱里它只有文件名/伪造路径，
 * 拿不到真实绝对路径——Tauri 会拦截原生拖放并经 IPC 把 `paths: string[]`
 * 递到 JS，这是唯一可靠通道。
 *
 * 约定：drop 时回调首个满足 filter 的路径；非 Tauri 环境（浏览器 dev）自动
 * 跳过订阅，调用方保持手填输入框可用。`enabled` 供页面按需暂停（如切走 tab）。
 */
export function useDragDropPath(opts: {
  onPath: (path: string) => void;
  /** 文件扩展名白名单（小写、不带点）；空/缺省 = 不限 */
  extensions?: string[];
  enabled?: boolean;
}): void {
  const { onPath, extensions, enabled = true } = opts;
  // 调用方常传字面量数组（每次 render 新引用）；序列化为稳定依赖，
  // 避免 effect 频繁重建导致反复订阅/退订 webview 拖放事件
  const extKey = extensions?.join(",") ?? "";
  useEffect(() => {
    if (!enabled || !("__TAURI_INTERNALS__" in window)) return;
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void (async () => {
      try {
        const { getCurrentWebview } = await import("@tauri-apps/api/webview");
        const fn = await getCurrentWebview().onDragDropEvent((event) => {
          if (event.payload.type !== "drop") return;
          for (const p of event.payload.paths) {
            const exts = extKey ? extKey.split(",") : [];
          if (exts.length === 0) {
              onPath(p);
              return;
            }
            const lower = p.toLowerCase();
            if (exts.some((ext) => lower.endsWith(`.${ext}`))) {
              onPath(p);
              return;
            }
          }
        });
        if (disposed) fn();
        else unlisten = fn;
      } catch {
        // 旧版本无此 API：静默降级，手填仍可用
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [onPath, extKey, enabled]);
}
