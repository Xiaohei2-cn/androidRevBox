import { useEffect } from "react";
import { windowApi } from "@/api/window";
import { isSolidAt } from "@/lib/clickThrough";

/**
 * 指针轮询间隔。80ms 够"鼠标移上去就能点"，又不至于白烧 CPU：
 * 每 tick 就一次 IPC + 一次 elementFromPoint。
 */
const POLL_MS = 80;

/**
 * 透明区点击穿透：鼠标底下没有实体时把整窗设成穿透，点击就落到后面的 App 上；
 * 底下是卡片/按钮/文字/窗口 chrome 就照常接。
 *
 * **这不是设置项**（用户口径：默认能力，别在设置里占一行开关）。形状化的透明窗口本来就
 * 有一圈不属于任何元素的留白，默认接住点击等于在那儿摆一块看不见的挡板；判定见
 * `lib/clickThrough`：窗口边缘拉伸热区、标题栏、齿块本体永远算实体，所以"穿"出去的
 * 只有本来就没东西的地方——不会出现"界面还在但点不动"。
 *
 * 为什么不是 CSS `pointer-events`：macOS 的命中测试不看像素 alpha，元素再透明也是
 * 我们的窗口在收事件；而 `pointer-events:none` 只能把点击交给**同一个 webview 里
 * 更下层的元素**，交不出窗口外面。
 *
 * 组件卸载、页面隐藏、IPC 出错这三条路都必须立刻收回穿透状态：撂着不管的最坏形状是
 * "整个软件点不动，而退出按钮也在点不动的范围里"。
 */
export function useClickThrough(): void {
  useEffect(() => {
    // 浏览器里（vitest / vite 预览）没有窗口层，直接不启
    if (!("__TAURI_INTERNALS__" in window)) return;
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
        if (cancelled) return;
        if (!state.inside) {
          // 光标在别处：不改状态。鼠标都不在窗里，穿不穿透都跟它要点的东西无关，
          // 来回切换只会让状态抖
          return;
        }
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
  }, []);
}
