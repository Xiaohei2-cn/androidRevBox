import { useCallback, useEffect, useRef, useState } from "react";

/**
 * 算法清单 -> 算法组合的**指针拖拽**。
 *
 * 为什么不用 HTML5 DnD（`draggable` + dataTransfer）：
 * 1. `dragover` 阶段浏览器（含 WKWebView）**故意不让你读 `dataTransfer.getData()`**，
 *    只有 `types` 可见。上一版就是在 dragover 里调 getData 判断"是不是算法拖来的"，
 *    永远返回空串 -> 从不 `preventDefault()` -> `drop` 根本不触发，表现就是"拖不动"。
 * 2. 更要命的是本 App 开着 Tauri 的 `dragDropEnabled`（SO 替换与托管页靠它接住访达拖入的
 *    文件），macOS 上原生拖放目标会进一步吞掉页面内的 HTML5 拖拽。为了文件拖入不能关它。
 * 所以这里只依赖 Pointer Events：几何判定自己算，跟 webview 的 DnD 设置无关。
 *
 * 也不画跟随光标的"小卡片"：那要 `transform`/绝对定位层，透明窗口里会被 WebKit 提成
 * 合成层，层创建销毁那一帧满 alpha = 整窗闪一下（见 globals.css 第三轮闪烁修复）。
 * 反馈走既有那条虚线插入槽，够用且不闪。
 */
const PICK_THRESHOLD = 5;

/**
 * 拖拽期间挂在全局类名上禁选 + 统一光标。
 *
 * 为什么只在条目上写 `select-none` 不够：WebKit 起选区时会把锚点落到**最近的可选文本**上，
 * 指针一 Move 就延伸进"算法组合"面板，那边照样被涂蓝（实测就是用户报的现象）。
 * 所以整次手势期间要在 document 级别关掉 user-select，松手/取消再恢复。
 * `cursor: grabbing` 同时给出"我正拖着东西"的反馈（改光标不建合成层，安全）。
 */
const DRAG_CLASS = "chef-dragging";

function setDragging(on: boolean) {
  try {
    document.documentElement.classList.toggle(DRAG_CLASS, on);
  } catch {
    // 没有 document 的环境（不该发生）直接跳过，不阻塞算法操作
  }
}

export interface PickDragApi {
  /** 正在拖的算法 id；null = 没在拖 */
  pickingId: string | null;
  /** 落点下标（插在第几步前面）；null = 指针不在接受区里 */
  dropIndex: number | null;
  /** 清单条目按下时调用 */
  startPick: (id: string, event: React.PointerEvent) => void;
  /** 算法组合面板挂上去，用来做落点几何判定 */
  setArea: (el: HTMLElement | null) => void;
  cancel: () => void;
  /** 读取并清空"这次手势其实是拖拽"标记（给 click 用） */
  consumeDrag: () => boolean;
}

export function usePickDrag(
  visibleStepCount: number,
  onDrop: (id: string, index: number) => void,
): PickDragApi {
  const [pickingId, setPickingId] = useState<string | null>(null);
  const [dropIndex, setDropIndex] = useState<number | null>(null);
  const areaRef = useRef<HTMLElement | null>(null);
  const draggedRef = useRef(false);

  const cancel = useCallback(() => {
    setPickingId(null);
    setDropIndex(null);
    setDragging(false);
  }, []);

  const startPick = useCallback((id: string, event: React.PointerEvent) => {
    // 只挡右键/中键（右键要留给上下文菜单）。写成 > 0 而不是 !== 0：
    // jsdom 没有 PointerEvent 构造器，测试里 button 可能是 undefined，
    // `!== 0` 那种写法会让拖拽在测试环境下永远不启动（真实浏览器反而正常，最难查）。
    if (event.button !== undefined && event.button > 0) return;
    draggedRef.current = false;
    // 按下就禁选：不能等到超过阈值，那之前 WebKit 已经开始涂选区了
    setDragging(true);
    // 顺手清掉页面上**已存在**的选区：留着它，浏览器可能改成"拖这段选中的文字"，
    // 表现同样是我们收不到 pointermove 且屏幕上一块蓝跟着鼠标走。
    try {
      window.getSelection()?.removeAllRanges();
    } catch {
      // 极少数环境不暴露 selection，忽略即可
    }
    // 把这次指针的后续事件锁在条目上：全局 user-select:text 会让 WebKit 起文本选区，
    // 一旦升级为原生拖拽，pointermove/pointerup 就不再看我们（表现：一动就没反应）。
    const el = event.currentTarget as HTMLElement;
    if (typeof el.setPointerCapture === "function" && event.pointerId !== undefined) {
      try {
        el.setPointerCapture(event.pointerId);
      } catch {
        // 指针可能已经不存在（极少数触屏场景），不阻塞主流程
      }
    }
    // 按下不等于要拖：先挂着一次性的阈值监听，真拖了才进入 picking 态。
    // 必须命令式加监听——startPick 不触发渲染，挂在 effect 里永远轮不到执行。
    const origin = { x: event.clientX, y: event.clientY };
    const controller = new AbortController();
    const move = (next: PointerEvent) => {
      if (Math.hypot(next.clientX - origin.x, next.clientY - origin.y) < PICK_THRESHOLD) return;
      draggedRef.current = true;
      controller.abort();
      setPickingId(id);
    };
    const settle = () => {
      controller.abort();
      // 没超过阈值就是普通点击：静默取消，把选区权限还给页面
      if (!draggedRef.current) setDragging(false);
    };
    window.addEventListener("pointermove", move, { signal: controller.signal });
    window.addEventListener("pointerup", settle, { signal: controller.signal });
    window.addEventListener("pointercancel", settle, { signal: controller.signal });
  }, []);

  /** 拖完紧跟的那次 click 要吞掉，否则"拖一下"会变成"拖 + 多加一步" */
  const consumeDrag = useCallback(() => {
    const dragged = draggedRef.current;
    draggedRef.current = false;
    return dragged;
  }, []);

  /** 指针在哪个位置松手 = 插在哪一步前面；不在接受区内返回 null */
  const indexAt = useCallback(
    (x: number, y: number): number | null => {
      const area = areaRef.current;
      if (!area) return null;
      const box = area.getBoundingClientRect();
      if (x < box.left || x > box.right || y < box.top || y > box.bottom) return null;
      const rows = area.querySelectorAll<HTMLElement>("[data-step-index]");
      for (const row of rows) {
        const index = Number(row.dataset.stepIndex);
        const rect = row.getBoundingClientRect();
        if (y < rect.top) return index;
        if (y <= rect.bottom) return y < rect.top + rect.height / 2 ? index : index + 1;
      }
      return rows.length > 0 ? Number(rows[rows.length - 1].dataset.stepIndex) + 1 : 0;
    },
    [],
  );

  useEffect(() => {
    if (!pickingId) return;
    // AbortController 统一收线：pointerup/pointercancel/Escape 任一条走完就全拆掉
    const controller = new AbortController();
    const move = (event: PointerEvent) => {
      const next = indexAt(event.clientX, event.clientY);
      // 只在落点真的变化时 setState：一路鼠标移动不该每帧重排配方面板
      setDropIndex((previous) => (previous === next ? previous : next));
    };
    const finish = (event: PointerEvent) => {
      const target = indexAt(event.clientX, event.clientY);
      const id = pickingId;
      cancel();
      if (target !== null) onDrop(id, target);
    };
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") cancel();
    };
    window.addEventListener("pointermove", move, { signal: controller.signal });
    window.addEventListener("pointerup", finish, { signal: controller.signal });
    window.addEventListener("pointercancel", cancel, { signal: controller.signal });
    window.addEventListener("keydown", key, { signal: controller.signal });
    return () => controller.abort();
  }, [pickingId, indexAt, cancel, onDrop]);

  // 步骤数变了（比如刚插入），落点下标要跟着钳一下，否则高亮会停在不存在的位置
  useEffect(() => {
    setDropIndex((previous) =>
      previous === null ? previous : Math.max(0, Math.min(previous, visibleStepCount)),
    );
  }, [visibleStepCount]);

  useEffect(() => () => setDragging(false), []);

  const setArea = useCallback((el: HTMLElement | null) => {
    areaRef.current = el;
  }, []);

  return { pickingId, dropIndex, startPick, setArea, cancel, consumeDrag };
}
