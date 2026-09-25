import { useEffect } from "react";
import { windowApi } from "@/api/window";
import { isSolidAt } from "@/lib/clickThrough";

/**
 * 指针轮询间隔。80ms 够"鼠标移上去就能点"，又不至于白烧 CPU：
 * 每 tick 就一次 IPC + 一次 elementFromPoint。
 */
const POLL_MS = 80;

/**
 * 透明区点击穿透（第五十四轮）：鼠标底下没有实体时，把整窗设成穿透，
 * 点击就落到后面的 App 上；底下是卡片/按钮/文字/窗口 chrome 就照常接。
 *
 * 为什么不是 CSS `pointer-events`：macOS 的命中测试不看像素 alpha，
 * 元素再透明也是我们的窗口在收事件；而 `pointer-events:none` 只能把点击
 * 交给**同一个 webview 里更下层的元素**，交不出窗口外面。
 *
 * 关掉开关、组件卸载、页面隐藏三条路都必须立刻收回穿透状态：
 * 撂着不管的最坏形状是"整个软件点不动，而退出按钮也在点不动的范围里"。
 */
export function useClickThrough(enabled: boolean): void {
  useEffect(() => {
    // 浏览器里（vitest / vite 预览）没有窗口层，直接不启
    if (!enabled || !("__TAURI_INTERNALS__" in window)) return;
    let ignore = false;
    let cancelled = false;

    const restore = () => {
      if (!ignore) return;
      ignore = false;
      void windowApi.setClickThrough(false).catch(() => undefined);
    };

    const tick = async () => {
      if (cancelled) return;
      try {
        if (document.hidden) {
          restore();
          return;
        }
        const state = await windowApi.pointerState();
        if (cancelled || !state.inside) return;
        const next = !isSolidAt(state.x, state.y);
        if (next === ignore) return;
        await windowApi.setClickThrough(next);
        ignore = next;
      } catch {
        // 问不到指针就当"可点击"：宁可这次没穿透，也不能把自己界面锁死
        restore();
      }
    };

    const timer = window.setInterval(() => void tick(), POLL_MS);
    void tick();
    return () => {
      cancelled = true;
      window.clearInterval(timer);
      restore();
    };
  }, [enabled]);
}
