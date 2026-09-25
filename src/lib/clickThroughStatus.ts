/**
 * 穿透运行状态（第五十四轮返工）：给设置页显示的一条实时读数。
 *
 * 为什么要它：用户报"透传没生效"时，"开关没生效"、"循环没在跑"、"判定认为那儿是实体"
 * 这三种故障从界面上长得一模一样。有这条读数才能一眼分清是哪一种。
 */
export type ClickThroughStatus =
  /** 开关关着（默认） */
  | "off"
  /** 开着，但光标不在本窗口内 */
  | "outside"
  /** 开着，光标在实体上 → 本工具接点击 */
  | "solid"
  /** 开着，光标在透明处 → 点击给后面的 App */
  | "passing"
  /** 开着，但问不到指针（IPC/窗口层出错）→ 保守按可点击处理 */
  | "error";

let current: ClickThroughStatus = "off";
const listeners = new Set<(status: ClickThroughStatus) => void>();

export function getClickThroughStatus(): ClickThroughStatus {
  return current;
}

export function setClickThroughStatus(next: ClickThroughStatus): void {
  if (next === current) return;
  current = next;
  for (const listener of listeners) listener(next);
}

export function subscribeClickThroughStatus(listener: (status: ClickThroughStatus) => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}
