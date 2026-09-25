import { describe, expect, it } from "vitest";

import {
  PAINT_ALPHA,
  alphaOf,
  domJudges,
  isSolidAt,
  isSolidStack,
} from "./clickThrough";

/** 造一个元素栈：jsdom 没有布局引擎，`elementsFromPoint` 用不了，所以直接喂栈 */
function el(opts: {
  className?: string;
  tag?: string;
  bg?: string;
  clickThrough?: "solid" | "pass";
  role?: string;
}): Element {
  const node = document.createElement(opts.tag ?? "div");
  if (opts.className) node.className = opts.className;
  if (opts.bg) node.style.backgroundColor = opts.bg;
  if (opts.clickThrough) node.setAttribute("data-click-through", opts.clickThrough);
  if (opts.role) node.setAttribute("role", opts.role);
  return node;
}

const solid = (stack: Element[]) => isSolidStack(stack, domJudges());

describe("透明区点击穿透的判定", () => {
  it("只有半透明底色与布局容器 → 透明，让点击出去", () => {
    expect(
      solid([
        el({ className: "absolute inset-0 p-5" }),
        el({ className: "app-surface", bg: "rgba(24, 24, 27, 0.55)" }),
        el({ className: "page-canvas", bg: "rgba(17, 24, 39, 0.03)" }),
      ]),
    ).toBe(false);
  });

  it("压在底色上面的卡片算实体", () => {
    expect(
      solid([
        el({ className: "rounded-xl bg-card", bg: "rgb(255 255 255 / 1)" }),
        el({ className: "app-surface", bg: "rgba(24, 24, 27, 0.55)" }),
      ]),
    ).toBe(true);
  });

  it("极浅着色（低于阈值）不算实体，别把 3% 的层次当成卡片", () => {
    expect(solid([el({ bg: "rgba(255 255 255 / 0.04)" })])).toBe(false);
    expect(solid([el({ bg: `rgba(255 255 255 / ${PAINT_ALPHA})` })])).toBe(true);
  });

  it("原生控件与带 role 的东西一律算实体，哪怕它没画背景", () => {
    expect(solid([el({ tag: "button" })])).toBe(true);
    expect(solid([el({ tag: "input" })])).toBe(true);
    expect(solid([el({ role: "tab" })])).toBe(true);
  });

  it("控件优先于底色层：齿块带着 .app-surface 也仍是实体（选中态点得动）", () => {
    // 这是本轮返工的根因之一：`.app-surface` 同时挂在窗口底色与 tab 齿块上，
    // 先判底色会把实心不透明的选中齿块也当成"可穿透"，当前页的 tab 就点丢了。
    const selectedTooth = el({ tag: "button", className: "app-surface bg-primary" });
    expect(solid([selectedTooth])).toBe(true);
  });

  it("显式 pass 的透明齿块让出点击，但图标热区在上面就先算实体", () => {
    const tooth = el({ tag: "button", className: "app-surface", clickThrough: "pass" });
    // 齿块本体（只有半透明着色）→ 穿透，这正是用户要的那块
    expect(solid([tooth, el({ className: "app-surface" })])).toBe(false);
    // 图标热区压在齿块上面 → 仍然切得了页
    const pad = el({ clickThrough: "solid" });
    expect(solid([pad, tooth, el({ className: "app-surface" })])).toBe(true);
  });

  it("窗口 chrome 显式钉成实体：标题栏永远点得回来", () => {
    expect(solid([el({ clickThrough: "solid", className: "app-surface" })])).toBe(true);
    // 反过来也必须成立：显式声明穿透的层优先于任何启发式
    expect(solid([el({ clickThrough: "pass", bg: "rgb(0 0 0 / 1)" })])).toBe(false);
  });

  it("越靠前越上面：先碰到实体就停，不去看下面的底色", () => {
    expect(
      solid([el({ clickThrough: "solid" }), el({ clickThrough: "pass" })]),
    ).toBe(true);
  });

  it("alphaOf 覆盖现代/老式两种写法与关键字", () => {
    expect(alphaOf("rgba(24, 24, 27, 0.6)")).toBeCloseTo(0.6);
    expect(alphaOf("rgb(255 255 255 / 0.25)")).toBeCloseTo(0.25);
    expect(alphaOf("hsl(0 0% 0% / 35%)")).toBeCloseTo(0.35);
    expect(alphaOf("rgb(17 24 39)")).toBe(1);
    expect(alphaOf("transparent")).toBe(0);
    expect(alphaOf("")).toBe(0);
    // 认不出来时按不透明处理：宁可这次没穿透，也不能把界面点不动
    expect(alphaOf("not-a-color")).toBe(1);
  });

  it("拿不到元素栈时算实体（无布局环境/坐标越界都不能把界面点不动）", () => {
    expect(isSolidAt(10, 10)).toBe(true);
  });
});
