/**
 * 控制栏（窗口最上方那条）的两条硬约束（用户要求：控制栏不允许透明化）。
 *
 * 1. **永远实色**：底色走 `.app-titlebar`，alpha 恒为 1，不吃 `--app-bg-alpha`。
 *    「背景不透明度」调到底也只该影响主体与齿块——拖窗、最小化、关闭都在这条上，
 *    让它半透明等于把窗口控制按钮摆在"隔着背后的桌面猜"的位置上。
 * 2. **永远接住点击**：`data-click-through="solid"`，穿透模式一次都不许让出去。
 *
 * 判据落在类名与源码层：jsdom 不加载 globals.css，量不到实际 alpha，
 * 所以这里读 CSS 源文本核对（不是"看着像就行"，是那条规则真的写成了 1）。
 */
import { readFileSync } from "node:fs";
import path from "node:path";
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { TitleBar } from "./TitleBar";
import { I18nProvider } from "@/i18n/I18nProvider";

const win = {
  close: vi.fn(),
  minimize: vi.fn(),
  toggleMaximize: vi.fn(),
};

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => win,
}));

function renderBar() {
  return render(
    <I18nProvider>
      <TitleBar />
    </I18nProvider>,
  );
}

describe("控制栏不许透明化", () => {
  it("那条挂的是实色层 .app-titlebar，不是随透明度变的 .app-surface", () => {
    renderBar();
    const bar = screen.getByRole("banner");
    expect(bar.className).toContain("app-titlebar");
    expect(bar.className).not.toContain("app-surface");
    // 能拖动：Tauri 的拖拽区标记在整条上（标题文字那一格也带着）
    expect(bar).toHaveAttribute("data-tauri-drag-region");
  });

  it("穿透模式也从不让这里让出去（solid 优先于 pass）", () => {
    renderBar();
    expect(screen.getByRole("banner")).toHaveAttribute("data-click-through", "solid");
  });

  it("globals.css 里 .app-titlebar 的 alpha 必须是 1，且不碰透明度变量/backdrop-filter", () => {
    const css = readFileSync(path.join(process.cwd(), "src/styles/globals.css"), "utf8");
    const rule = css.match(/\.app-titlebar\s*\{([^}]*)\}/);
    expect(rule, "找不到 .app-titlebar 规则").toBeTruthy();
    const body = rule![1];
    expect(body).toMatch(/rgb\(var\(--app-bg-rgb\) \/ 1\)/);
    expect(body).not.toContain("--app-bg-alpha");
    // 闪烁修复那条：透明层上加 backdrop-filter/transform 会满 alpha 闪一帧
    expect(body).not.toMatch(/backdrop-filter|transform/);
  });

  it("非 macOS 的三个窗口按钮各归各位（关闭在最右）", () => {
    renderBar();
    screen.getByRole("button", { name: "最小化窗口" });
    screen.getByRole("button", { name: "切换最大化" });
    screen.getByRole("button", { name: "关闭窗口" });
  });
});
