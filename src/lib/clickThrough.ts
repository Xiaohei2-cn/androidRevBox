/**
 * 透明区点击穿透的判定层。
 *
 * 规则是**反过来**的：默认所有地方都算实体、都接点击；只有被显式标成 `data-click-through="pass"`
 * 的区域（左侧 tab 栏的透明留白与齿块本体）才把点击让给后面的 App。
 *
 * 为什么不做"看谁透明"的启发式（我第一版就是这么写的，被用户当场指出两个后果）：
 * ① 内容区里卡片之间的间隙、3% 的极浅着色会被判成"透明"，于是**点自己的界面点到了后面**；
 * ② 无边框窗口的拉伸热区正好压在边缘那几个透明像素上，穿透一开**整窗就拖不动了**。
 * 猜错了的代价是"自己的界面失灵"，所以宁可少穿、不可乱穿。
 */

/** 显式声明"这里可以让出去" */
export const PASS_ATTR = "pass";
/** 显式声明"这里必须接住"（优先级高于 pass，用于 pass 区域里的控件热区） */
export const SOLID_ATTR = "solid";

/**
 * 窗口边缘抓住区（CSS px）：`decorations:false` 的窗口只能靠边缘热区拉伸，
 * 这几像素永远算实体，否则"透明处穿透"会把调整窗口大小一起废掉。
 */
export const EDGE_GRIP_PX = 12;

export interface Viewport {
  width: number;
  height: number;
}

/** 是不是压在窗口边缘的拉伸热区上 */
export function isOnEdgeGrip(x: number, y: number, viewport: Viewport): boolean {
  return (
    x <= EDGE_GRIP_PX ||
    y <= EDGE_GRIP_PX ||
    x >= viewport.width - EDGE_GRIP_PX ||
    y >= viewport.height - EDGE_GRIP_PX
  );
}

/**
 * 从上往下找第一个带声明的元素：`solid` 接住、`pass` 让出；一路都没声明 → **接住**。
 * 纯函数，不需要布局引擎，也不需要读 computed style（那种读法正是上一版误判的来源）。
 */
export function isSolidStack(
  elements: readonly Element[],
  viewport: Viewport = { width: 0, height: 0 },
  point = { x: -1, y: -1 },
): boolean {
  // 拉伸优先于任何让渡声明：贴着边缘时哪怕在 pass 区域里也绝不能把点击送出去
  if (isOnEdgeGrip(point.x, point.y, viewport)) return true;
  for (const el of elements) {
    const mark = el.getAttribute("data-click-through");
    if (mark === SOLID_ATTR) return true;
    if (mark === PASS_ATTR) return false;
  }
  return true;
}

/**
 * 这一点该不该接点击。
 *
 * 拿不到元素栈（坐标在视口外、无布局环境）时也按"接住"处理：宁可这次没穿透，
 * 也不能出现"看着界面在眼前、点下去却打到后面那个 App"。
 */
export function isSolidAt(
  x: number,
  y: number,
  doc: Document = document,
  viewport: Viewport = {
    width: doc.defaultView?.innerWidth ?? 0,
    height: doc.defaultView?.innerHeight ?? 0,
  },
): boolean {
  const stack = typeof doc.elementsFromPoint === "function" ? doc.elementsFromPoint(x, y) : [];
  if (!stack || stack.length === 0) return true;
  return isSolidStack(stack, viewport, { x, y });
}
