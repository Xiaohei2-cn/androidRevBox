import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { AppNavProvider } from "@/app/nav";
import { DICTIONARIES } from "@/i18n/dictionaries";
import { I18nProvider } from "@/i18n/I18nProvider";
import { MAIN_TABS, MainTabRail } from "./MainTabRail";

const ZH_LABELS = DICTIONARIES["zh-CN"];

function renderRail() {
  return render(
    <TooltipProvider>
      <AppNavProvider>
        <I18nProvider>
          <MainTabRail />
        </I18nProvider>
      </AppNavProvider>
    </TooltipProvider>,
  );
}

describe("MainTabRail", () => {
  it("渲染全部 13 个总 tab（默认 zh-CN 标签）", () => {
    renderRail();
    for (const tab of MAIN_TABS) {
      expect(screen.getByRole("button", { name: ZH_LABELS[tab.labelKey] })).toBeInTheDocument();
    }
    expect(MAIN_TABS).toHaveLength(13);
  });

  it("点击 tab 切换激活态（context 驱动）", async () => {
    renderRail();
    // 初始激活：仪表盘（Provider 默认 tab）
    expect(screen.getByRole("button", { name: "仪表盘" })).toHaveAttribute(
      "aria-current",
      "true",
    );
    await userEvent.click(screen.getByRole("button", { name: "设备" }));
    const devices = screen.getByRole("button", { name: "设备" });
    expect(devices).toHaveAttribute("aria-current", "true");
    expect(devices).toHaveTextContent("设备");
    expect(screen.getByRole("button", { name: "仪表盘" })).not.toHaveAttribute(
      "aria-current",
    );
  });

  it("齿块整体必须接住点击：穿透只给齿块左边的留白", () => {
    // 用户实测：图标下半部分点下去穿到了后面的 App —— 因为上一版把未选中齿块本体标成
    // 穿透，只在图标外留 24×24 热区，而齿块是 36×40：图标下面正好空着 8px。
    // 点 tab 是基本操作，这种"差 8px 就穿"的设计不成立，所以齿块整体收回为实体。
    renderRail();
    const nav = screen.getByRole("navigation");
    expect(nav.getAttribute("data-click-through")).toBeNull();

    const teeth = screen.getAllByRole("button");
    expect(teeth).toHaveLength(13);
    for (const tooth of teeth) {
      // 齿块自己不标，且它的整条祖先链里也不能有 pass —— 否则"点哪里都是这个 tab"就不成立
      expect(tooth.getAttribute("data-click-through")).toBeNull();
      for (let node: HTMLElement | null = tooth.parentElement; node; node = node.parentElement) {
        expect(node.getAttribute("data-click-through")).not.toBe("pass");
      }
      // 图标也不该再被包一层"热区"（那种小热区正是这次事故的形状）。
      // 选中的 tab 显示的是中文标签、没有图标，所以只在有图标时断言。
      const icon = tooth.querySelector("svg");
      if (icon) expect(icon.parentElement).toBe(tooth);
    }

    // 唯一的穿透区：每行齿块左边那条留白带；行容器与 nav 都没有声明
    const rows = nav.querySelectorAll(":scope > div");
    expect(rows).toHaveLength(13);
    for (const row of rows) {
      expect(row.getAttribute("data-click-through")).toBeNull();
      const strips = row.querySelectorAll('span[data-click-through="pass"]');
      expect(strips).toHaveLength(1);
      expect(strips[0].className).toContain("flex-1"); // 剩下的宽度全归留白，齿块多宽都不错位
      expect(row.contains(strips[0])).toBe(true);
      // 留白带排在齿块左边（它先渲染，齿块靠右）
      expect(row.firstElementChild).toBe(strips[0]);
      expect(row.lastElementChild?.tagName).toBe("BUTTON");
    }
    // 顶部那 56px 是 nav 的 padding（没有声明 → 接住），不额外做元素：
    // 做成 flex 子元素会被 nav 的 gap 多推 6px，整条栏与内容区顶边错位
    expect(nav.className).toContain("pt-14");
    expect(nav.querySelectorAll(":scope > span[data-click-through=\"pass\"]")).toHaveLength(0);
  });

  it("当前激活 tab 标记 aria-current", () => {
    renderRail();
    const dashboard = screen.getByRole("button", { name: "仪表盘" });
    expect(dashboard).toHaveAttribute("aria-current", "true");
    const settings = screen.getByRole("button", { name: "设置" });
    expect(settings).not.toHaveAttribute("aria-current");
  });
});
