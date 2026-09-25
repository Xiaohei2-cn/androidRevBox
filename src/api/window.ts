import { invokeCommand } from "./client";

/**
 * 光标在本窗口内的位置（逻辑/CSS 像素，与 `elementFromPoint` 同一坐标系）。
 * 换算在 Rust 侧做：物理像素、Retina 缩放、副屏负坐标都在那儿，前端不该猜。
 */
export interface PointerState {
  x: number;
  y: number;
  /** 光标落没落在窗口矩形内。不在时前端**不改**穿透状态：鼠标在别处，
   * 我们这儿穿不穿透都跟它要点的东西无关，来回切换只会让状态抖。 */
  inside: boolean;
}

export const windowApi = {
  /** 取指针位置（Rust 侧同时把它当作前端心跳，超时会自动收回穿透状态） */
  pointerState: (): Promise<PointerState> =>
    invokeCommand<PointerState>("window_pointer_state"),
  /**
   * 整窗穿透开关：`true` = 点击落到后面的 App。
   * 返回真正落下去的值（不拿"我以为设上了"当事实）。
   */
  setClickThrough: (ignore: boolean): Promise<boolean> =>
    invokeCommand<boolean>("window_set_click_through", { ignore }),
};
