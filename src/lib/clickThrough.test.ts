import { describe, expect, it } from "vitest";

import {
  EDGE_GRIP_PX,
  isOnEdgeGrip,
  isSolidAt,
  isSolidStack,
} from "./clickThrough";

function el(mark?: "solid" | "pass", className = ""): HTMLElement {
  const node = document.createElement(className === "button" ? "button" : "div");
  if (mark) node.setAttribute("data-click-through", mark);
  return node;
}

const VIEWPORT = { width: 1200, height: 800 };
const CENTER = { x: 600, y: 400 };

describe("点击穿透的判定（默认接住，只有显式声明才让出）", () => {
  it("什么都不标 → 接住点击", () => {
    // 用户报的第一条：内容区卡片之间的间隙、3% 极浅着色被上一版猜成"透明"，
    // 结果点自己的界面打到了后面的 App。默认必须反过来。
    expect(isSolidStack([el(), el(), document.createElement("html")], VIEWPORT, CENTER)).toBe(true);
  });

  it("显式 pass 才让出点击", () => {
    expect(isSolidStack([el(), el("pass")], VIEWPORT, CENTER)).toBe(false);
  });

  it("pass 区域里的控件热区仍然算实体（tab 图标就是这一类）", () => {
    // 栈是从上往下的：图标 pad(solid) 压在齿块(pass) 与 rail(pass) 上面
    expect(isSolidStack([el("solid"), el("pass"), el("pass")], VIEWPORT, CENTER)).toBe(true);
    // 而热区之外就是让出去
    expect(isSolidStack([el(), el("pass"), el("pass")], VIEWPORT, CENTER)).toBe(false);
  });

  it("拿不到元素栈时算实体：宁可没穿透，不能把界面点失灵", () => {
    expect(isSolidAt(10, 10, { elementsFromPoint: undefined, defaultView: null } as unknown as Document)).toBe(true);
    expect(isSolidAt(10, 10, { elementsFromPoint: () => [], defaultView: null } as unknown as Document)).toBe(true);
  });
});

describe("窗口边缘的拉伸热区", () => {
  it("四条边各 EDGE_GRIP_PX 内都算抓住区", () => {
    expect(isOnEdgeGrip(2, 400, VIEWPORT)).toBe(true);
    expect(isOnEdgeGrip(600, 1, VIEWPORT)).toBe(true);
    expect(isOnEdgeGrip(VIEWPORT.width - 3, 400, VIEWPORT)).toBe(true);
    expect(isOnEdgeGrip(600, VIEWPORT.height - 3, VIEWPORT)).toBe(true);
    expect(isOnEdgeGrip(600, 400, VIEWPORT)).toBe(false);
    expect(EDGE_GRIP_PX).toBeGreaterThan(0);
  });

  it("边缘优先于 pass 声明：不然无边框窗口一开穿透就拖不动、也拉伸不了", () => {
    // 用户报的第二条，正是这个形状：点击被透传到下面的 App，界面尺寸调不了
    expect(isSolidStack([el("pass")], VIEWPORT, { x: 3, y: 400 })).toBe(true);
  });
});
